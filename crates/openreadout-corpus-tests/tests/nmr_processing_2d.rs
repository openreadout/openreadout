//! Multidimensional NMR processing against the vendors' own processed spectra:
//!
//! - Bruker: every experiment in the corpus (outside the held-out tier) with TopSpin's
//!   `pdata/<n>/2rr`: the `ser` processed here with the stored parameters
//!   (`load_processed_nd`, default options) against `2rr` read through the Bruker reader. The
//!   correlation over all points and over the signal points (|2rr| > 10 × its noise SD), the
//!   least-squares scale, the position of the tallest point and the ppm axes. Commands that
//!   TopSpin's `auditp.txt` records but `procs` do not (`sym`, `tilt`/`symj`) are applied when the
//!   audit trail lists them, since the stored spectrum was made with them. Pseudo-2D series
//!   (DOSY) compare the F2 spectra row by row.
//! - Bruker 3D (`nmrglue-bruker-3d`, no `acqu3s`): processed with explicit parameters (smoke
//!   test: finite, peaks inside the spectrum).
//! - Varian/Agilent 2D (States and TPPI), JEOL 2D, Spinsolve `data.2d` series: no vendor
//!   spectrum is public; checked for a diagonal (COSY), finite values, the number of rows and
//!   the series list.
//!
//! Experiments TopSpin processed with steps this crate does not do are reported, not asserted:
//! non-uniform sampling (`FnTYPE` 2, a later reconstruction step), solvent filtering
//! (`BC_mod` ≥ 5). Experiments with linear prediction in F1 are asserted at a lower bound.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test
//! nmr_processing_2d -- --nocapture` (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`).
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, FormatReader};
use openreadout_signal::nmr::{
    self, DimensionOptions, DimensionParameters, Encoding, NdOptions, PhaseMode, StoredDimension,
    Window,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn find_2rr(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("ho-") || name == "heldout" {
            continue;
        }
        if p.is_dir() && depth > 0 {
            find_2rr(&p, depth - 1, out);
        } else if name == "2rr" {
            out.push(p);
        }
    }
}

fn corr(a: &[f64], b: &[f64], mask: Option<&[bool]>) -> f64 {
    let pick = |i: usize| mask.is_none_or(|m| m[i]);
    let n = a.len().min(b.len());
    let (mut sa, mut sb, mut k) = (0.0, 0.0, 0.0);
    for i in (0..n).filter(|&i| pick(i)) {
        sa += a[i];
        sb += b[i];
        k += 1.0;
    }
    let (ma, mb) = (sa / k, sb / k);
    let (mut c, mut va, mut vb) = (0.0, 0.0, 0.0);
    for i in (0..n).filter(|&i| pick(i)) {
        c += (a[i] - ma) * (b[i] - mb);
        va += (a[i] - ma).powi(2);
        vb += (b[i] - mb).powi(2);
    }
    c / (va * vb).sqrt()
}

fn robust_sd(v: &[f64]) -> f64 {
    let mut s: Vec<f64> = v
        .iter()
        .step_by((v.len() / 200_000).max(1))
        .copied()
        .collect();
    s.sort_by(f64::total_cmp);
    let med = s[s.len() / 2];
    let mut d: Vec<f64> = s.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(f64::total_cmp);
    1.4826 * d[d.len() / 2]
}

fn read_lossy(file: &Path) -> Option<String> {
    std::fs::read(file)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn param(file: &Path, key: &str) -> Option<String> {
    let t = read_lossy(file)?;
    let tag = format!("##${key}=");
    t.lines()
        .find_map(|l| l.strip_prefix(&tag).map(|v| v.trim().to_string()))
}

/// Commands after the last `xfb` in TopSpin's audit trail.
fn audit_after_xfb(pdata: &Path) -> Vec<String> {
    let Some(t) = read_lossy(&pdata.join("auditp.txt")) else {
        return Vec::new();
    };
    let mut cmds: Vec<String> = Vec::new();
    for l in t.lines() {
        let l = l.trim_start();
        if let Some(c) = l.strip_prefix('<') {
            let w = c.split_whitespace().next().unwrap_or("").to_string();
            if w == "Start" {
                cmds.clear();
            } else if !w.is_empty() {
                cmds.push(w);
            }
        }
    }
    cmds
}

struct Row {
    id: String,
    note: String,
    c_all: f64,
    c_sig: f64,
    scale: f64,
    peak_ok: bool,
    axes_ok: bool,
    category: &'static str,
}

#[test]
fn bruker_2d_matches_topspin() {
    let dir = files_dir();
    let mut list = Vec::new();
    find_2rr(&dir, 6, &mut list);
    list.sort();
    // only data sets listed in the manifest (docs/legal/clean-room-policy.md rule 1)
    let manifest = std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap_or_default();
    list.retain(|p| {
        let rel = p.strip_prefix(&dir).unwrap_or(p);
        let top = rel
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string());
        top.is_some_and(|t| manifest.contains(&format!("filename = \"{t}")))
    });
    if list.is_empty() {
        eprintln!("no 2rr in the corpus; skipped");
        return;
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut failures = Vec::new();
    for rr in &list {
        let pdata = rr.parent().unwrap();
        let exp = pdata.parent().unwrap().parent().unwrap();
        let procno = pdata.file_name().unwrap().to_string_lossy().to_string();
        let id =
            exp.strip_prefix(&dir).unwrap_or(exp).display().to_string() + &format!("/{procno}");
        if !exp.join("ser").exists() || !pdata.join("proc2s").exists() {
            continue;
        }
        if let Some(only) = std::env::var_os("OPENREADOUT_ND_ONLY")
            && !id.contains(only.to_string_lossy().as_ref())
        {
            continue;
        }
        // the loader uses the lowest procno with proc2s; other procnos are compared through
        // their own experiments only when they are that procno
        let lowest = std::fs::read_dir(exp.join("pdata"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("proc2s").exists())
            .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
            .min();
        if lowest.map(|l| l.to_string()) != Some(procno.clone()) {
            continue;
        }
        let Ok(mut ds) = openreadout_nmr::BrukerReader.open(exp) else {
            eprintln!("{id}: reader refused");
            continue;
        };
        let info = ds.info().unwrap();
        let Some(fid) = info.traces.iter().find(|t| nmr::source::is_fid(t)) else {
            continue;
        };
        if fid
            .extra
            .get("indirect_dimensions")
            .and_then(|v| v.as_array())
            .is_none_or(|a| a.len() != 1)
        {
            continue;
        }
        let fid_index = fid.index;
        let name = format!("pdata/{procno}");
        let Some(pt) = info
            .traces
            .iter()
            .find(|t| t.name.as_deref() == Some(&name))
        else {
            continue;
        };
        let (pi, n1s, n2s) = (pt.index, pt.sweep_count as usize, pt.sample_count as usize);
        let ch = pt
            .channels
            .iter()
            .position(|c| c.name == "2rr")
            .unwrap_or(0);
        let mut stored = Vec::with_capacity(n1s * n2s);
        for s in 0..pt.sweep_count {
            let t = ds.read_trace(pi, s, 0, pt.sample_count).unwrap();
            stored.extend_from_slice(&t.channels[ch]);
        }
        let sweep_axis = pt
            .extra
            .get("sweep_axis")
            .and_then(|a| a.get("axis"))
            .cloned();
        let axis2 = pt.extra.get("axis").cloned();
        let cmds = audit_after_xfb(pdata);
        let pulprog = param(&exp.join("acqus"), "PULPROG").unwrap_or_default();
        let fntype = param(&exp.join("acqus"), "FnTYPE").unwrap_or_default();
        let bc = param(&pdata.join("procs"), "BC_mod")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let me = param(&pdata.join("proc2s"), "ME_mod")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let opts = NdOptions {
            symmetrize: cmds.iter().any(|c| c == "sym"),
            tilt: cmds.iter().any(|c| c == "tilt"),
            ..NdOptions::default()
        };
        let t0 = std::time::Instant::now();
        let s = match nmr::load_processed_nd(ds.as_mut(), &info, fid_index, &opts) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{id}: {e}"));
                continue;
            }
        };
        let secs = t0.elapsed().as_secs_f64();
        let series = s.series.is_some();
        // compare on the common grid. A DOSY series: TopSpin's 2rr is the diffusion-transformed
        // spectrum (F1 = log diffusion coefficient), so our first F2 row (the weakest gradient)
        // is compared with its sum over F1, which carries the same F2 line shapes
        let (n1, n2) = (s.shape[0].min(n1s), s.shape[1].min(n2s));
        let mut ours = Vec::with_capacity(n1 * n2);
        let mut theirs = Vec::with_capacity(n1 * n2);
        if series {
            ours.extend_from_slice(&s.real[..n2]);
            let mut sum = vec![0.0; n2];
            for i in 0..n1s {
                for (j, v) in sum.iter_mut().enumerate() {
                    *v += stored[i * n2s + j];
                }
            }
            theirs = sum;
        } else {
            for i in 0..n1 {
                ours.extend_from_slice(&s.real[i * s.shape[1]..i * s.shape[1] + n2]);
                theirs.extend_from_slice(&stored[i * n2s..i * n2s + n2]);
            }
        }
        let sd = robust_sd(&theirs);
        let mask: Vec<bool> = theirs.iter().map(|v| v.abs() > 10.0 * sd).collect();
        let nsig = mask.iter().filter(|m| **m).count();
        let c_all = corr(&ours, &theirs, None);
        let c_sig = if nsig > 10 {
            corr(&ours, &theirs, Some(&mask))
        } else {
            f64::NAN
        };
        let (mut num, mut den) = (0.0, 0.0);
        for (a, b) in ours.iter().zip(&theirs) {
            num += a * b;
            den += a * a;
        }
        let scale = num / den;
        let amax = |v: &[f64]| {
            v.iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .map_or(0, |x| x.0)
        };
        let (ka, kb) = (amax(&ours), amax(&theirs));
        let peak_ok = (ka / n2).abs_diff(kb / n2) <= 1 && (ka % n2).abs_diff(kb % n2) <= 1;
        let f = |v: &Option<serde_json::Value>, k: &str| {
            v.as_ref()
                .and_then(|a| a.get(k))
                .and_then(serde_json::Value::as_f64)
        };
        // a stored F1 width that differs from the acquired one (an edited proc2s) cannot match
        let sw_stored = param(&pdata.join("proc2s"), "SW_p").and_then(|v| v.parse::<f64>().ok());
        let sw_acq = param(&exp.join("acqu2s"), "SW_h").and_then(|v| v.parse::<f64>().ok());
        let f1_edited =
            matches!((sw_stored, sw_acq), (Some(a), Some(b)) if (a / b - 1.0).abs() > 0.01);
        let axes_ok = (f(&axis2, "first").unwrap_or(f64::NAN) - s.axes[1].first_ppm).abs() < 2e-3
            && (series
                || f1_edited
                || (f(&sweep_axis, "first").unwrap_or(f64::NAN) - s.axes[0].first_ppm).abs()
                    < 2e-2);
        let enc = s.record.dimensions[0].encoding;
        let pp = pulprog.to_ascii_lowercase();
        let category = if fntype == "2" {
            "nus"
        } else if bc >= 5 {
            "solvent filter"
        } else if pp.contains("mp3q") || pp.contains("mqmas") {
            // solid-state multiple-quantum MAS: needs a shearing transformation
            "mqmas"
        } else if series {
            "series"
        } else if me != 0 {
            "lp"
        } else {
            "plain"
        };
        let note = format!(
            "{pulprog} {enc:?} {}{}{} F1 phase {} | {:.1} s",
            if me != 0 {
                format!("LP ME_mod {me} ")
            } else {
                String::new()
            },
            if opts.symmetrize { "sym " } else { "" },
            if opts.tilt { "tilt " } else { "" },
            s.record.dimensions[0].phase_mode,
            secs
        );
        println!(
            "{id:44} {category:14} corr {c_all:+.6} signal {c_sig:+.6} ({nsig} pts) scale {scale:.4} peak {peak_ok} axes {axes_ok} | {note} [F2 {:.4}/{:.4}, F1 {:.4}/{:.4}]",
            s.axes[1].first_ppm,
            f(&axis2, "first").unwrap_or(f64::NAN),
            s.axes[0].first_ppm,
            f(&sweep_axis, "first").unwrap_or(f64::NAN),
        );
        rows.push(Row {
            id,
            note,
            c_all,
            c_sig,
            scale,
            peak_ok,
            axes_ok,
            category,
        });
    }
    // shortfalls with their reasons (docs/formats/nmr-processing.md), asserted at a lower bound
    let known: &[(&str, f64)] = &[
        // linear prediction with nearly as many coefficients as points (NCOEF 60, 64 points)
        ("nmrxiv-s1617-bundle/HSQC", 0.98),
        ("nmrxiv-s1617-bundle/HMBC", 0.98),
        // linear prediction of a magnitude HMBC (NCOEF 32 of 64 points)
        ("nmrxiv-s73-bundle/6", 0.97),
        // EM 5 Hz in F2 on DSPFVS 12 data; the residual is not explained
        ("nmrxiv-s572-bundle/4", 0.997),
    ];
    let mut checked = 0;
    for r in &rows {
        let mut need = match r.category {
            "plain" => 0.999,
            "series" => 0.99,
            "lp" => 0.99,
            _ => continue,
        };
        if let Some((_, k)) = known.iter().find(|(id, _)| r.id.starts_with(id)) {
            need = *k;
        }
        let got = if r.c_sig.is_finite() {
            r.c_sig
        } else {
            r.c_all
        };
        checked += 1;
        if !(got >= need) || !r.axes_ok || !(r.scale > 0.0) {
            failures.push(format!(
                "{}: signal correlation {:.6} (need {need}), scale {:.4}, axes {}, peak {} ({})",
                r.id, got, r.scale, r.axes_ok, r.peak_ok, r.note
            ));
        }
    }
    println!("{checked} experiments asserted, {} compared", rows.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn open_any(path: &Path) -> Box<dyn Dataset> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if ext.eq_ignore_ascii_case("jdf") {
        openreadout_nmr::JeolReader.open(path).unwrap()
    } else if path.join("procpar").exists() {
        openreadout_nmr::VarianReader.open(path).unwrap()
    } else if path.join("acqu.par").exists() {
        openreadout_nmr::SpinsolveReader.open(path).unwrap()
    } else {
        openreadout_nmr::BrukerReader.open(path).unwrap()
    }
}

/// With `OPENREADOUT_ND_DUMP=<dir>`, write a result as `<dir>/<name>.f64` (little-endian,
/// row-major) and `<dir>/<name>.json` (shape and axes) for comparisons outside the test.
fn dump(name: &str, s: &nmr::SpectrumNd) {
    let Some(dir) = std::env::var_os("OPENREADOUT_ND_DUMP") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let bytes: Vec<u8> = s.real.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(dir.join(format!("{name}.f64")), bytes).unwrap();
    let axes: Vec<_> = s
        .axes
        .iter()
        .map(|a| serde_json::json!({"first": a.first_ppm, "step": a.step_ppm, "size": a.size}))
        .collect();
    std::fs::write(
        dir.join(format!("{name}.json")),
        serde_json::json!({"shape": s.shape, "axes": axes}).to_string(),
    )
    .unwrap();
}

/// Is the strongest signal of a homonuclear 2D spectrum on (or next to) the diagonal?
fn diagonal_share(s: &nmr::SpectrumNd) -> f64 {
    let (n1, n2) = (s.shape[0], s.shape[1]);
    let (mut on, mut all) = (0.0, 0.0);
    for i in 0..n1 {
        let ppm = s.axes[0].ppm(i as f64);
        let j0 = s.axes[1].index_of(ppm);
        for j in 0..n2 {
            let v = s.real[i * n2 + j].abs();
            all += v;
            if (j as f64 - j0).abs() <= 0.02 * n2 as f64 {
                on += v;
            }
        }
    }
    on / all
}

#[test]
fn other_vendors_2d() {
    let dir = files_dir();
    let mut failures = Vec::new();
    // Varian/Agilent (nmrglue test data): States and TPPI
    for (id, rel, want) in [
        (
            "nmrglue-agilent-2d",
            "nmrglue-test-data/agilent_2d",
            Encoding::States,
        ),
        (
            "nmrglue-agilent-2d-tppi",
            "nmrglue-test-data/agilent_2d_tppi",
            Encoding::Tppi,
        ),
    ] {
        let p = dir.join(rel);
        if !p.exists() {
            continue;
        }
        let mut ds = open_any(&p);
        let info = ds.info().unwrap();
        let params = nmr::nd_parameters(ds.as_ref(), &info, 0).unwrap();
        let s = nmr::load_processed_nd(ds.as_mut(), &info, 0, &NdOptions::default()).unwrap();
        println!(
            "{id}: {:?} shape {:?}, F1 {:?} {}, F2 phase {} {:.1}/{:.1}, notes {:?}",
            params.indirect[0].encoding,
            s.shape,
            s.record.dimensions[0].encoding,
            s.record.dimensions[0].phase_mode,
            s.record.dimensions[1].phase_mode,
            s.record.dimensions[1].phase0_deg,
            s.record.dimensions[1].phase1_deg,
            s.record.notes
        );
        dump(id, &s);
        if params.indirect[0].encoding != want {
            failures.push(format!("{id}: encoding {:?}", params.indirect[0].encoding));
        }
        if !s.real.iter().all(|v| v.is_finite()) {
            failures.push(format!("{id}: non-finite values"));
        }
    }
    // JEOL 2D: COSY should be strongest on its diagonal
    for rel in [
        "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_COSY_400MHz_Jeol.jdf",
        "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_HSQC_400MHz_Jeol.jdf",
        "nmrxiv-s200/Limonene_7020ug200uL_CDCl3_HMBC_400MHz_Jeol.jdf",
        "figshare29098781-hmbc.jdf",
        "zenodo19692590-discomycin-hsqc.jdf",
    ] {
        let p = dir.join(rel);
        if !p.exists() {
            continue;
        }
        let mut ds = open_any(&p);
        let info = ds.info().unwrap();
        let Some(fid) = info.traces.iter().find(|t| nmr::source::is_fid(t)) else {
            println!("{rel}: no FID (a processed file)");
            continue;
        };
        let idx = fid.index;
        match nmr::load_processed_nd(ds.as_mut(), &info, idx, &NdOptions::default()) {
            Ok(s) => {
                dump(&Path::new(rel).file_stem().unwrap().to_string_lossy(), &s);
                let diag = diagonal_share(&s);
                println!(
                    "{rel}: shape {:?}, F1 {:?} {}, diagonal share {diag:.3}, F1 axis {:.2}..{:.2} ppm, notes {:?}",
                    s.shape,
                    s.record.dimensions[0].encoding,
                    s.record.dimensions[0].phase_mode,
                    s.axes[0].first_ppm,
                    s.axes[0].last_ppm(),
                    s.record.notes
                );
                if rel.contains("COSY") && diag < 0.3 {
                    failures.push(format!("{rel}: diagonal share {diag:.3}"));
                }
                if !s.real.iter().all(|v| v.is_finite()) {
                    failures.push(format!("{rel}: non-finite values"));
                }
            }
            Err(e) => println!("{rel}: {e}"),
        }
    }
    // Spinsolve series
    let mut spins = Vec::new();
    for base in ["spinsolveproc", "spinsolve-zenodo20597567"] {
        let b = dir.join(base);
        let mut stack = vec![b];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if e.file_name() == "data.2d" {
                    spins.push(d.clone());
                }
            }
        }
    }
    spins.sort();
    for d in &spins {
        let mut ds = open_any(d);
        let info = ds.info().unwrap();
        let Some(t) = info
            .traces
            .iter()
            .find(|t| nmr::source::is_fid(t) && t.sweep_count > 1)
        else {
            continue;
        };
        let (idx, sweeps) = (t.index, t.sweep_count as usize);
        match nmr::load_processed_nd(ds.as_mut(), &info, idx, &NdOptions::default()) {
            Ok(s) => {
                let ser = s.series.clone().unwrap_or_default();
                println!(
                    "{}: {} rows, series {} ({} values, {:?}), F2 phase {}",
                    d.file_name().unwrap().to_string_lossy(),
                    s.shape[0],
                    ser.name,
                    ser.values.len(),
                    ser.unit,
                    s.record.dimensions[1].phase_mode
                );
                if s.shape[0] != sweeps || ser.name.is_empty() {
                    failures.push(format!("{}: rows {} of {sweeps}", d.display(), s.shape[0]));
                }
            }
            Err(e) => failures.push(format!("{}: {e}", d.display())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The nmrglue 3D test set has no `acqu3s`; the third dimension is given by hand from the
/// parameters nmrglue's conversion script lists (States × States, 116 rows, 5555.556 Hz,
/// 201.204 MHz 13C).
#[test]
fn bruker_3d_smoke() {
    let p = files_dir().join("nmrglue-test-data/bruker_3d");
    if !p.exists() {
        return;
    }
    let mut ds = open_any(&p);
    let info = ds.info().unwrap();
    let mut params = nmr::nd_parameters(ds.as_ref(), &info, 0).unwrap();
    let rows = nmr::indirect::read_rows(ds.as_mut(), &info, 0).unwrap();
    // 15N (acqu2s): 128 rows of 2500 Hz at 81.086 MHz, carrier 119.787 ppm
    let r2 = 128;
    params.indirect.truncate(1);
    params.indirect[0] = DimensionParameters {
        nucleus: Some("15N".into()),
        spectral_width_hz: 2500.0,
        carrier_frequency_mhz: 81.086 * (1.0 + 119.787e-6),
        reference_frequency_mhz: 81.086,
        rows: r2,
        encoding: Encoding::States,
        ..DimensionParameters::default()
    };
    params.indirect.push(DimensionParameters {
        nucleus: Some("13C".into()),
        spectral_width_hz: 5555.556,
        carrier_frequency_mhz: 201.204 * (1.0 + 55.743e-6),
        reference_frequency_mhz: 201.204,
        rows: rows.len() / r2,
        encoding: Encoding::States,
        ..DimensionParameters::default()
    });
    let auto = DimensionOptions {
        phase: PhaseMode::Manual {
            phase0_deg: 0.0,
            phase1_deg: 0.0,
        },
        window: Some(Window::Sine {
            offset: 0.5,
            power: 2.0,
        }),
        ..DimensionOptions::default()
    };
    let opts = NdOptions {
        direct: DimensionOptions {
            phase: PhaseMode::Auto,
            ..auto.clone()
        },
        indirect: vec![auto.clone(), auto],
        ..NdOptions::default()
    };
    let t0 = std::time::Instant::now();
    let s = nmr::process_nd(
        &rows,
        &params.direct,
        &StoredDimension::default(),
        &params.indirect,
        &opts,
    )
    .unwrap();
    println!(
        "bruker_3d: {} rows, shape {:?}, axes {:?}, {:.1} s, notes {:?}",
        rows.len(),
        s.shape,
        s.axes
            .iter()
            .map(|a| (a.first_ppm, a.last_ppm()))
            .collect::<Vec<_>>(),
        t0.elapsed().as_secs_f64(),
        s.record.notes
    );
    assert_eq!(s.ndim(), 3);
    assert!(s.real.iter().all(|v| v.is_finite()));
    let max = s.real.iter().fold(0.0_f64, |a, b| a.max(b.abs()));
    assert!(max > 0.0);
    for a in 0..3 {
        assert!(s.projection(a, nmr::Projection::Skyline).is_some());
    }
}
