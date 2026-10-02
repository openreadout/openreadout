//! Reads the SYNTHETIC HDF5 fixtures in `tests/fixtures/` (written with h5py by
//! `make_fixtures.py`, cross-read by h5py into `tests/fixtures/oracle/*.json` by `oracle/gen.py`).

use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_hdf5::{Hdf5Reader, ImsReader, NwbReader};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn oracle(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("oracle").join(format!("{name}.json"))).unwrap(),
    )
    .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(ImsReader))
        .with(Box::new(NwbReader))
        .with(Box::new(Hdf5Reader))
}

fn f64_hash(v: &[f64]) -> String {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes))
}

#[test]
fn detection_prefers_the_specific_readers() {
    let reg = registry();
    for (f, id) in [
        ("synthetic-imaris.ims", "ims"),
        ("synthetic-timeseries.nwb", "nwb"),
        ("synthetic-generic.h5", "hdf5"),
    ] {
        let (_, d) = reg.detect(&fixtures().join(f)).unwrap();
        assert_eq!(d.format_id, id, "{f}");
    }
    // a renamed Imaris file is still recognised by its root attributes
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("renamed.h5");
    std::fs::copy(fixtures().join("synthetic-imaris.ims"), &p).unwrap();
    assert_eq!(reg.detect(&p).unwrap().1.format_id, "ims");
}

#[test]
fn imaris_matches_h5py() {
    let o = oracle("synthetic-imaris");
    let mut ds = ImsReader
        .open(&fixtures().join("synthetic-imaris.ims"))
        .unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    let oi = &o["images"][0];
    assert_eq!(
        (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t),
        (37, 21, 5, 2, 2)
    );
    assert_eq!(u64::from(im.size_x), oi["size_x"].as_u64().unwrap());
    assert_eq!(im.pixel_type.ome_name(), oi["pixel_type"]);
    assert!((im.physical_size.x.unwrap() - 0.5).abs() < 1e-9);
    assert!((im.physical_size.z.unwrap() - 2.0).abs() < 1e-9);
    assert_eq!(im.time_increment_s, Some(2.5));
    assert_eq!(im.channels[1].name.as_deref(), Some("GFP"));
    assert_eq!(im.channels[1].color.as_deref(), Some("#00FF00"));
    assert_eq!(im.channels[0].excitation_nm, Some(405.0));
    assert_eq!(im.acquired_at.as_deref(), Some("2026-01-02T03:04:05.678"));
    assert!(info.notes.iter().any(|n| n.contains("time zone")));
    assert_eq!(im.objective.as_ref().unwrap().lens_na, Some(1.3));
    assert_eq!(im.pyramid_levels, 2);
    let mut n = 0;
    for p in oi["planes"].as_array().unwrap() {
        let idx = PlaneIndex {
            c: p["c"].as_u64().unwrap() as u32,
            z: p["z"].as_u64().unwrap() as u32,
            t: p["t"].as_u64().unwrap() as u32,
        };
        assert_eq!(
            ds.read_plane(0, idx).unwrap().xxh3_hex(),
            p["xxh3"],
            "{idx:?}"
        );
        n += 1;
    }
    assert_eq!(n, 20);
    let l = &oi["levels"][0];
    let plane = ds.read_plane_level(0, PlaneIndex::default(), 1).unwrap();
    assert_eq!((plane.width, plane.height), (19, 11));
    assert_eq!(plane.xxh3_hex(), l["planes"][0]["xxh3"]);
    assert!(ds.check().unwrap().ok);
    assert!(
        ds.entries()
            .unwrap()
            .iter()
            .any(|e| e.kind == "pyramid-level" && e.details["filters"][0] == "shuffle")
    );
    // out of range
    assert_eq!(
        ds.read_plane(0, PlaneIndex { c: 2, z: 0, t: 0 })
            .unwrap_err()
            .exit_code(),
        2
    );
}

#[test]
fn truncated_imaris_is_a_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = std::fs::read(fixtures().join("synthetic-imaris.ims")).unwrap();
    for cut in [bytes.len() / 3, bytes.len() * 2 / 3, bytes.len() - 100] {
        let p = dir.path().join(format!("cut{cut}.ims"));
        std::fs::write(&p, &bytes[..cut]).unwrap();
        match ImsReader.open(&p) {
            Err(e) => assert!(matches!(e.exit_code(), 4..=6), "{e}"),
            Ok(mut ds) => {
                // metadata survived: reading everything must fail cleanly or check must flag it
                let r = ds.check().unwrap();
                assert!(!r.ok, "cut at {cut}: check passed");
            }
        }
    }
}

#[test]
fn nwb_traces_match_h5py() {
    let o = oracle("synthetic-timeseries");
    let mut ds = NwbReader
        .open(&fixtures().join("synthetic-timeseries.nwb"))
        .unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("2.7.0"));
    assert!(info.notes.iter().any(|n| n.contains("Test Lab")));
    let traces = o["traces"].as_array().unwrap();
    assert_eq!(info.traces.len(), traces.len());
    for ot in traces {
        let i = ot["index"].as_u64().unwrap() as u32;
        let t = &info.traces[i as usize];
        assert_eq!(t.name.as_deref(), ot["name"].as_str());
        let names: Vec<&str> = t.channels.iter().map(|c| c.name.as_str()).collect();
        let onames: Vec<&str> = ot["channel_names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(names, onames);
        let rate = ot["sample_rate_hz"].as_f64().unwrap();
        assert!(
            (t.sample_rate_hz - rate).abs() <= 1e-9 * rate.max(1.0),
            "{}",
            t.sample_rate_hz
        );
        let sw = &ot["sweeps"][0];
        let n = sw["sample_count"].as_u64().unwrap();
        assert_eq!(t.sample_count, n);
        let data = ds.read_trace(i, 0, 0, n).unwrap();
        for (c, oc) in sw["channels"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                f64_hash(&data.channels[c]),
                oc["xxh3"],
                "trace {i} channel {c}"
            );
        }
    }
    // the regular series: rate from starting_time, offset applied
    let voltage = info
        .traces
        .iter()
        .position(|t| t.name.as_deref() == Some("voltage"))
        .unwrap();
    assert_eq!(info.traces[voltage].start_s, Some(1.5));
    let part = ds.read_trace(voltage as u32, 0, 10, 5).unwrap();
    assert_eq!(part.first_sample, 10);
    assert_eq!(part.channels[0].len(), 5);
    // the ElectricalSeries without a time base is read, and `check` says so
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "time_base"));
}

/// Compare every oracle table (row count, column names, per-column xxh3 of the values as f64).
fn tables_match(ds: &mut Box<dyn openreadout_core::Dataset>, o: &Value) {
    let info = ds.info().unwrap();
    let tables = o["tables"].as_array().unwrap();
    assert_eq!(info.tables.len(), tables.len());
    for ot in tables {
        let i = ot["index"].as_u64().unwrap() as u32;
        let t = &info.tables[i as usize];
        assert_eq!(t.name.as_deref(), ot["name"].as_str());
        let rows = ot["event_count"].as_u64().unwrap();
        assert_eq!(t.row_count, rows, "table {i}");
        let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        let onames: Vec<&str> = ot["parameter_names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(names, onames, "table {i}");
        let tab = ds.read_table(i, 0, rows).unwrap();
        for (c, name) in names.iter().enumerate() {
            assert_eq!(
                f64_hash(&tab.columns[c]),
                ot["column_hashes"][name],
                "table {i} column {name}"
            );
        }
    }
}

#[test]
fn nwb_ecephys_matches_h5py() {
    let o = oracle("synthetic-ecephys");
    let mut ds = NwbReader
        .open(&fixtures().join("synthetic-ecephys.nwb"))
        .unwrap();
    let info = ds.info().unwrap();
    let raw = &info.traces[0];
    assert_eq!(raw.name.as_deref(), Some("raw"));
    assert_eq!(raw.extra["neurodata_type"], "ElectricalSeries");
    // electrodes region [3, 0, 2]: channel 0 is electrode row 3 (id 13, location CA1)
    let e = &raw.channels[0].extra;
    assert_eq!(e["electrode_row"], 3);
    assert_eq!(e["electrode"]["id"], 13.0);
    assert_eq!(e["electrode"]["location"], "CA1");
    // channel_conversion [1.0, 0.5, 2.0] multiplies the conversion
    assert!((raw.channels[1].scale - 1e-6 * 0.5).abs() < 1e-18);
    for ot in o["traces"].as_array().unwrap() {
        let i = ot["index"].as_u64().unwrap() as u32;
        let sw = &ot["sweeps"][0];
        let data = ds
            .read_trace(i, 0, 0, sw["sample_count"].as_u64().unwrap())
            .unwrap();
        for (c, oc) in sw["channels"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                f64_hash(&data.channels[c]),
                oc["xxh3"],
                "trace {i} channel {c}"
            );
        }
    }
    let lfp = &info.traces[1];
    assert_eq!(lfp.extra["path"], "/processing/ecephys/LFP/LFP");
    tables_match(&mut ds, &o);
    // text columns are category codes; ragged columns are counts; the spike-times table maps
    // each spike to its unit
    let units = &info.tables[0];
    let q = units
        .columns
        .iter()
        .position(|c| c.name == "quality")
        .unwrap();
    assert_eq!(
        units.columns[q].extra["categories"],
        serde_json::json!(["good", "noise"])
    );
    let spikes = ds.read_table(1, 2, 3).unwrap();
    assert_eq!(spikes.columns[0], [0.0, 1.0, 2.0]);
    assert_eq!(spikes.columns[1], [7.0, 8.0, 9.0]);
    let electrodes = &info.tables[3];
    assert!(
        electrodes.extra["skipped_columns"]
            .to_string()
            .contains("group")
    );
    assert!(ds.check().unwrap().ok);
}

#[test]
fn generic_hdf5_lists_the_tree() {
    let mut ds = Hdf5Reader
        .open(&fixtures().join("synthetic-generic.h5"))
        .unwrap();
    let ls = ds.entries().unwrap();
    let names: Vec<&str> = ls.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "/",
            "/labels",
            "/results",
            "/results/run1",
            "/results/run1/matrix"
        ]
    );
    assert_eq!(ls[0].details["attributes"]["title"], "plain HDF5");
    assert_eq!(ls[4].details["shape"], serde_json::json!([3, 4]));
    assert_eq!(ls[4].details["dtype"], "float32");
    assert_eq!(ls[3].details["attributes"]["operator"], "nobody");
    let info = ds.info().unwrap();
    assert!(info.images.is_empty());
    assert!(info.notes[0].starts_with("3 groups and 2 datasets"));
    assert_eq!(
        ds.read_plane(0, PlaneIndex::default())
            .unwrap_err()
            .exit_code(),
        6
    );
    assert!(ds.check().unwrap().ok);
}
