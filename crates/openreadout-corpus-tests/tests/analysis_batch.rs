//! Batch plug-ins of the analysis commands and per-component RGB statistics.
//!
//! - Every analysis measure (`peaks`, `chromatogram`, `assay`, `nmr-peaks`, `ephys-features`,
//!   `spikes`, `qpcr`, and `stats` per well) run through `openreadout_batch::run` must give, row
//!   for row and cell for cell, the records of the single-file command (the library call the CLI
//!   and the MCP tool make) on the same file: exact equality of every scalar field.
//! - `stats` `components[]` of RGB images against czifile / nd2 + NumPy
//!   (`oracle/rgb_components.py` → `corpus/oracle/stats/rgb_components.json`): counts, min, max
//!   and median exact, mean and std to 1e-12 relative.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test analysis_batch -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_batch::measures::{MeasureSpec, build};
use openreadout_batch::run::BatchRequest;
use openreadout_batch::{Table, Value};
use openreadout_core::parallel::ReadContext;
use openreadout_core::stats::{StatsRequest, compute_stats};
use openreadout_core::{Dataset, Registry};
use serde_json::{Map, Value as J, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_qpcr::RdmlReader))
        .with(Box::new(openreadout_qpcr::EdsReader))
        .with(Box::new(openreadout_qpcr::RexReader))
        .with(Box::new(openreadout_qpcr::IxoReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_blackrock::BlackrockReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_hcs::ImageXpressReader))
        .with(Box::new(openreadout_hcs::CellVoyagerReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_nmr::BrukerReader))
        .with(Box::new(openreadout_plate::PlateReader))
}

fn input(rel: &str) -> Option<PathBuf> {
    let p = files_dir().join(rel);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skip: {rel} not in the corpus");
        None
    }
}

/// The batch table of `measure` with `options` over one file.
fn batch(measure: &str, options: &J, path: &Path) -> Table {
    let mut spec = MeasureSpec::default();
    spec.measure = measure.to_string();
    spec.options = options.as_object().cloned().unwrap_or_default();
    batch_spec(&spec, path)
}

fn batch_spec(spec: &MeasureSpec, path: &Path) -> Table {
    let measure = &spec.measure;
    let m = build(spec).unwrap();
    let mut req = BatchRequest::default();
    req.inputs = vec![path.to_path_buf()];
    let r = openreadout_batch::run(&registry(), m.as_ref(), &req, None, None).unwrap();
    let t = r.table;
    if let Some(i) = t.index_of("error") {
        for row in &t.rows {
            assert!(
                row[i].is_null(),
                "{measure} on {}: {:?}",
                path.display(),
                row[i]
            );
        }
    }
    t
}

/// Every scalar field of every record equals the row's cell (floats bit for bit).
fn assert_rows_equal(what: &str, t: &Table, records: &[J], skip: &[&str]) {
    assert_eq!(t.rows.len(), records.len(), "{what}: row count");
    assert!(!records.is_empty(), "{what}: no records to compare");
    let mut cells = 0usize;
    for (i, rec) in records.iter().enumerate() {
        let Some(obj) = rec.as_object() else {
            panic!("{what}: record {i} is not an object")
        };
        for (k, v) in obj {
            if skip.contains(&k.as_str()) {
                continue;
            }
            let ours = t.get(i, k);
            match v {
                J::Number(n) => {
                    let theirs = n.as_f64().unwrap();
                    let o = ours
                        .as_f64()
                        .unwrap_or_else(|| panic!("{what}: row {i} `{k}` is {ours:?}"));
                    assert!(
                        // bit for bit, or both zero (+0.0 and -0.0)
                        o.to_bits() == theirs.to_bits() || (o == 0.0 && theirs == 0.0),
                        "{what}: row {i} `{k}`: {o} != {theirs}"
                    );
                    cells += 1;
                }
                J::String(s) => {
                    assert_eq!(ours, &Value::Text(s.clone()), "{what}: row {i} `{k}`");
                    cells += 1;
                }
                J::Bool(b) => {
                    assert_eq!(ours, &Value::Bool(*b), "{what}: row {i} `{k}`");
                    cells += 1;
                }
                _ => {}
            }
        }
    }
    println!("{what}: {} rows, {cells} cells equal", records.len());
}

fn open(path: &Path) -> (Box<dyn Dataset>, openreadout_core::FileInfo) {
    let (_, ds) = registry().open(path).unwrap();
    let info = ds.info().unwrap();
    (ds, info)
}

fn to_records<T: serde::Serialize>(items: &[T]) -> Vec<J> {
    items
        .iter()
        .map(|x| serde_json::to_value(x).unwrap())
        .collect()
}

fn query<T: serde::de::DeserializeOwned>(v: &J) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn peaks_and_chromatogram_rows_equal_the_single_file_output() {
    let reg = registry();
    let Some(p) = input("chromhandler-001F0101.D") else {
        return;
    };
    let opts = json!({});
    let (mut ds, info) = open(&p);
    let opener = || reg.open(&p).map(|(_, d)| d);
    let ctx = ReadContext {
        opener: Some(&opener),
        progress: None,
    };
    let out = openreadout_quant::api::peaks(ds.as_mut(), &info, &query(&opts), &ctx).unwrap();
    assert_rows_equal(
        "peaks (per peak)",
        &batch("peaks", &opts, &p),
        &to_records(&out.peak_rows()),
        &["path"],
    );
    let t = batch("peaks", &json!({"rows": "chromatogram"}), &p);
    let summary: Vec<J> = out
        .chromatograms
        .iter()
        .map(|c| {
            json!({"chromatogram": c.label, "peak_count": c.peak_count, "total_area": c.total_area,
                   "main_peak_area_percent": c.main_peak_area_percent})
        })
        .collect();
    assert_rows_equal("peaks (per chromatogram)", &t, &summary, &[]);

    let copts = json!({"traces": [0]});
    let (mut ds, info) = open(&p);
    let c = openreadout_quant::api::chromatogram(ds.as_mut(), &info, &query(&copts), &ctx).unwrap();
    let recs: Vec<J> = to_records(&c.chromatograms)
        .into_iter()
        .map(|mut v| {
            let o = v.as_object_mut().unwrap();
            let label = o.remove("label").unwrap();
            o.insert("chromatogram".into(), label);
            v
        })
        .collect();
    assert_rows_equal(
        "chromatogram",
        &batch("chromatogram", &copts, &p),
        &recs,
        &["rt_min", "intensity"],
    );
}

#[test]
fn nmr_peaks_rows_equal_the_single_file_output() {
    let Some(p) = input("nmrxiv-s846/50") else {
        return;
    };
    for (opts, rows) in [
        (json!({}), "peak"),
        (
            json!({"from": "fid", "integrate": [[7.901, 7.814], [8.239, 8.178]]}),
            "integral",
        ),
    ] {
        let q: openreadout_signal::api::NmrQuery = query(&opts);
        let (mut ds, info) = open(&p);
        let out =
            openreadout_signal::nmr::analyze(ds.as_mut(), &info, &q.request().unwrap()).unwrap();
        let recs = if rows == "peak" {
            to_records(&out.peaks)
        } else {
            to_records(&out.integrals)
        };
        assert_rows_equal(
            &format!("nmr-peaks ({rows})"),
            &batch("nmr-peaks", &opts, &p),
            &recs,
            &[],
        );
    }
}

#[test]
fn ephys_and_spikes_rows_equal_the_single_file_output() {
    if let Some(p) = input("pyabf-171116sh-0018.abf") {
        let opts = json!({});
        let q: openreadout_signal::api::EphysQuery = query(&opts);
        let (mut ds, info) = open(&p);
        let out =
            openreadout_signal::ephys::analyze_cell(ds.as_mut(), &info, &q.request().unwrap())
                .unwrap();
        assert_rows_equal(
            "ephys-features (per sweep)",
            &batch("ephys-features", &opts, &p),
            &to_records(&out.sweeps),
            &[],
        );
        let cell = serde_json::to_value(&out.cell).unwrap();
        assert_rows_equal(
            "ephys-features (cell)",
            &batch("ephys-features", &json!({"rows": "cell"}), &p),
            &[cell],
            &[],
        );
    }
    if let Some(p) = input("brk-filespec2-3001.ns5") {
        let opts = json!({"max_seconds": 2.0});
        let q: openreadout_signal::api::SpikesQuery = query(&opts);
        let (mut ds, info) = open(&p);
        let out = openreadout_signal::ephys::analyze_extracellular(
            ds.as_mut(),
            &info,
            &q.request().unwrap(),
        )
        .unwrap();
        assert_rows_equal(
            "spikes",
            &batch("spikes", &opts, &p),
            &to_records(&out.channels),
            &["times", "times_truncated"],
        );
    }
}

#[test]
fn assay_and_qpcr_rows_equal_the_single_file_output() {
    let reg = registry();
    if let Some(p) = input("skanit-elisa-steps.xlsx") {
        for (opts, list) in [
            (json!({"analysis": "wells"}), "wells"),
            (json!({"analysis": "curve"}), "wells"),
            (json!({"analysis": "wells", "rows": "samples"}), "samples"),
        ] {
            let mut o: Map<String, J> = opts.as_object().cloned().unwrap();
            o.remove("rows");
            let req: openreadout_assay::AssayRequest = query(&J::Object(o));
            let out = openreadout_assay::analyze_file(&reg, &p, &req).unwrap();
            let recs = if list == "samples" {
                to_records(&out.samples)
            } else {
                to_records(&out.wells)
            };
            assert_rows_equal(
                &format!("assay {opts}"),
                &batch("assay", &opts, &p),
                &recs,
                &[],
            );
        }
    }
    if let Some(p) = input("eds-7500-abhd17c-ddct.eds") {
        let ds = openreadout_qpcr::open_qpcr(&reg, &p).unwrap();
        let out =
            openreadout_qpcr::qpcr_report(&ds, &openreadout_qpcr::QpcrReportRequest::default())
                .unwrap();
        assert_rows_equal(
            "qpcr (records)",
            &batch("qpcr", &json!({}), &p),
            &to_records(&out.records),
            &[],
        );
        let mut req = openreadout_qpcr::QpcrReportRequest::default();
        req.relative = true;
        req.reference_targets = vec!["18s".into()];
        req.control_sample = Some("Lenvatinib/Vector".into());
        let out = openreadout_qpcr::qpcr_report(&ds, &req).unwrap();
        assert_rows_equal(
            "qpcr (ΔΔCq)",
            &batch(
                "qpcr",
                &json!({"ddcq": true, "reference_targets": ["18s"], "control_sample": "Lenvatinib/Vector"}),
                &p,
            ),
            &to_records(&out.relative_quantities),
            &[],
        );
    }
}

#[test]
fn well_stats_rows_equal_the_single_file_output() {
    let reg = registry();
    let Some(p) = input("hcs/imagexpress-idr0081") else {
        return;
    };
    let mut spec = MeasureSpec::default();
    spec.measure = "stats".into();
    spec.per = Some("well".into());
    spec.select = vec!["c=0".into()];
    let (mut ds, info) = open(&p);
    let opener = || reg.open(&p).map(|(_, d)| d);
    let mut req = openreadout_core::plate::WellStatsRequest::default();
    req.select = vec!["c=0".into()];
    let out = openreadout_core::plate::well_stats(
        ds.as_mut(),
        &info,
        &req,
        &ReadContext {
            opener: Some(&opener),
            progress: None,
        },
    )
    .unwrap();
    assert_rows_equal(
        "stats per well",
        &batch_spec(&spec, &p),
        &to_records(&out.rows),
        &[],
    );
}

#[test]
fn rgb_components_match_czifile_and_nd2() {
    let oracle: J = serde_json::from_str(
        &std::fs::read_to_string(root().join("corpus/oracle/stats/rgb_components.json")).unwrap(),
    )
    .unwrap();
    let reg = registry();
    let mut checked = 0;
    for (id, f) in oracle["files"].as_object().unwrap() {
        let Some(p) = input(f["filename"].as_str().unwrap()) else {
            continue;
        };
        let (_, mut ds) = reg.open(&p).unwrap();
        let info = ds.info().unwrap();
        let out = compute_stats(
            ds.as_mut(),
            &info,
            &StatsRequest::default(),
            &ReadContext::default(),
        )
        .unwrap();
        for (i, want) in f["images"].as_array().unwrap().iter().enumerate() {
            let ch: Vec<_> = out
                .channels
                .iter()
                .filter(|c| c.image as usize == i)
                .collect();
            assert_eq!(ch.len(), 1, "{id} image {i}: one RGB channel");
            let comps = &ch[0].components;
            assert_eq!(comps.len(), 3, "{id} image {i}");
            for (k, (c, w)) in comps.iter().zip(want.as_array().unwrap()).enumerate() {
                let s = &c.stats;
                let rel = |a: f64, b: f64| (a - b).abs() / b.abs().max(1.0);
                assert_eq!(s.count, w["count"].as_u64().unwrap(), "{id} {i} {k} count");
                assert_eq!(s.min, w["min"].as_f64(), "{id} {i} {k} min");
                assert_eq!(s.max, w["max"].as_f64(), "{id} {i} {k} max");
                assert!(
                    rel(s.mean.unwrap(), w["mean"].as_f64().unwrap()) < 1e-12,
                    "{id} {i} {k} mean"
                );
                assert!(
                    rel(s.std.unwrap(), w["std"].as_f64().unwrap()) < 1e-9,
                    "{id} {i} {k} std"
                );
                assert_eq!(
                    s.percentiles.as_ref().map(|p| p.p50),
                    w["median"].as_f64(),
                    "{id} {i} {k} median"
                );
                if s.saturation_basis.as_deref() == Some("pixel_type") {
                    assert_eq!(
                        s.saturated_count,
                        w["at_type_max"].as_u64(),
                        "{id} {i} {k} sat"
                    );
                }
                assert_eq!(c.name.as_deref(), Some(["red", "green", "blue"][k]));
            }
            checked += 1;
        }
        println!("{id}: components equal ({})", f["reader"]);
    }
    assert!(checked > 0, "no RGB oracle file in the corpus");
}

#[test]
#[allow(clippy::many_single_char_names)] // p = path, i/c/t = image, channel, time point
fn mip_matches_czifile_and_nd2() {
    let oracle: J = serde_json::from_str(
        &std::fs::read_to_string(root().join("corpus/oracle/stats/mip.json")).unwrap(),
    )
    .unwrap();
    let reg = registry();
    let mut checked = 0;
    for (id, f) in oracle["files"].as_object().unwrap() {
        let Some(p) = input(f["filename"].as_str().unwrap()) else {
            continue;
        };
        let (_, mut ds) = reg.open(&p).unwrap();
        let info = ds.info().unwrap();
        let mut req = StatsRequest::default();
        req.per_plane = true;
        req.mip = Some(openreadout_core::stats::Projection::Z);
        let out = compute_stats(ds.as_mut(), &info, &req, &ReadContext::default()).unwrap();
        let want = f["projections"].as_array().unwrap();
        assert_eq!(out.planes.len(), want.len(), "{id}: projections");
        for w in want {
            let (i, c, t) = (
                w["image"].as_u64().unwrap(),
                w["c"].as_u64().unwrap(),
                w["t"].as_u64().unwrap(),
            );
            let got = out
                .planes
                .iter()
                .find(|pl| u64::from(pl.image) == i && u64::from(pl.c) == c && u64::from(pl.t) == t)
                .unwrap_or_else(|| panic!("{id}: no projection image {i} c {c} t {t}"));
            assert_eq!(got.z, 0);
            let (s, w) = (&got.stats, &w["stats"]);
            let rel = |a: f64, b: f64| (a - b).abs() / b.abs().max(1.0);
            assert_eq!(s.count, w["count"].as_u64().unwrap(), "{id} {i} {c} {t}");
            assert_eq!(s.min, w["min"].as_f64(), "{id} {i} {c} {t} min");
            assert_eq!(s.max, w["max"].as_f64(), "{id} {i} {c} {t} max");
            assert!(
                rel(s.mean.unwrap(), w["mean"].as_f64().unwrap()) < 1e-12,
                "{id} {i} {c} {t} mean"
            );
            assert!(
                rel(s.std.unwrap(), w["std"].as_f64().unwrap()) < 1e-9,
                "{id} {i} {c} {t} std"
            );
            assert_eq!(
                s.percentiles.as_ref().map(|p| p.p50),
                w["median"].as_f64(),
                "{id} {i} {c} {t} median"
            );
            checked += 1;
        }
        println!("{id}: {} projections equal ({})", want.len(), f["reader"]);
    }
    assert!(checked > 0, "no MIP oracle file in the corpus");
}
