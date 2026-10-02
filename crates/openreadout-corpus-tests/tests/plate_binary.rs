//! Plate-reader binary documents (SoftMax Pro `.pda`/`.sda`) beyond the per-mode value hashes
//! of `tests/corpus/`:
//! - `softmax5-kinetic-phage-120308b`: the depositor's export holds SoftMax Pro's plate-blank
//!   subtraction; our raw values minus the mean of the template's `Blank` group wells at each
//!   read reproduce every exported value (and time) within the export's last digit.
//! - every document with a text export: per plate, name, well values, temperature and the
//!   kinetic time grid equal the export read by the text reader.
//! - `softmax5-cuvette-spectra-s2` (cuvette-set spectra) is refused (exit 6), not guessed.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test plate_binary -- --nocapture`
#![cfg(feature = "corpus")]
// exact stored values are compared bit for bit; r/c/w/v/t/o name row, column, well, value, time, oracle
#![allow(clippy::float_cmp, clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

/// (row, col, time_s bits) -> value for every finite value of table `t`.
fn values(path: &Path, t: u32) -> BTreeMap<(u32, u32, u64), f64> {
    let mut ds = openreadout_plate::PlateReader.open(path).unwrap();
    let info = ds.info().unwrap();
    let n = info.tables[t as usize].row_count;
    let tab = ds.read_table(t, 0, n).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..tab.columns[0].len() {
        let v = tab.columns[6][i];
        if v.is_finite() {
            let time = tab.columns[5][i];
            let tb = if time.is_nan() {
                u64::MAX
            } else {
                time.to_bits()
            };
            out.insert(
                (
                    tab.columns[1][i] as u32 - 1,
                    tab.columns[2][i] as u32 - 1,
                    tb,
                ),
                v,
            );
        }
    }
    out
}

/// The exports print at most 6 decimals: half a unit of the 6th.
const EXPORT_DIGITS: f64 = 5.1e-7;

#[test]
fn documents_match_their_exports() {
    let pairs = [
        ("softmax5-kinetic-phage-120524", "pda"),
        ("softmax5-elisa-il10", "pda"),
        ("softmax5-elisa-tnf", "pda"),
        ("softmax7-lum-7skexp53", "sda"),
        ("softmax7-prestoblue-7skexp53", "sda"),
        ("softmax7-prestoblue-7skexp59", "sda"),
        ("softmax7-prestoblue-inhexp134", "sda"),
        ("softmax7-prestoblue-inhexp149", "sda"),
        ("softmax7-prestoblue-7skexp58", "sda"),
    ];
    let mut compared = 0;
    for (id, ext) in pairs {
        let doc = corpus_dir().join(format!("{id}.{ext}"));
        let txt = corpus_dir().join(format!("{id}.txt"));
        if !doc.exists() || !txt.exists() {
            eprintln!("skip {id}: missing");
            continue;
        }
        let d = openreadout_plate::PlateReader.open(&doc).unwrap();
        let t = openreadout_plate::PlateReader.open(&txt).unwrap();
        let (di, ti) = (d.info().unwrap(), t.info().unwrap());
        let d_tables: Vec<_> = di.tables.iter().collect();
        let t_tables: Vec<_> = ti
            .tables
            .iter()
            .filter(|t| !t.name.as_deref().unwrap_or("").is_empty())
            .collect();
        assert_eq!(d_tables.len(), t_tables.len(), "{id}: plate count");
        for (a, b) in d_tables.iter().zip(&t_tables) {
            assert_eq!(a.name, b.name, "{id}: plate names in order");
            let (va, vb) = (values(&doc, a.index), values(&txt, b.index));
            assert_eq!(va.len(), vb.len(), "{id} {:?}: value count", a.name);
            for (k, x) in &va {
                let y = vb
                    .get(k)
                    .unwrap_or_else(|| panic!("{id} {:?}: {k:?} not in the export", a.name));
                assert!(
                    (x - y).abs() <= EXPORT_DIGITS,
                    "{id} {:?} {k:?}: {x} vs export {y}",
                    a.name
                );
            }
            let (ta, tb) = (
                a.extra
                    .get("temperature_c")
                    .and_then(serde_json::Value::as_f64),
                b.extra
                    .get("temperature_c")
                    .and_then(serde_json::Value::as_f64),
            );
            if let (Some(ta), Some(tb)) = (ta, tb) {
                assert!((ta - tb).abs() <= 0.051, "{id}: temperature {ta} vs {tb}");
            }
            compared += va.len();
        }
    }
    eprintln!("{compared} values equal to their exports");
}

#[test]
fn plate_blank_subtraction_reproduces_the_export() {
    let doc = corpus_dir().join("softmax5-kinetic-phage-120308b.pda");
    let txt = corpus_dir().join("softmax5-kinetic-phage-120308b.txt");
    if !doc.exists() || !txt.exists() {
        eprintln!("skip: missing");
        return;
    }
    let ds = openreadout_plate::PlateReader.open(&doc).unwrap();
    let info = ds.info().unwrap();
    let layout = &info.tables[0].extra["layout"];
    let blanks: Vec<(u32, u32)> = layout["Role"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, v)| v.as_str() == Some("blank"))
        .map(|(w, _)| {
            let row = u32::from(w.as_bytes()[0] - b'A');
            let col: u32 = w[1..].parse().unwrap();
            (row, col - 1)
        })
        .collect();
    assert_eq!(blanks.len(), 8, "Blank group A12..H12");
    assert!(blanks.iter().all(|&(_, c)| c == 11));
    let raw = values(&doc, 0);
    let exported = values(&txt, 0);
    let mut times: Vec<u64> = raw.keys().map(|k| k.2).collect();
    times.sort_unstable();
    times.dedup();
    let mut n = 0;
    for t in times {
        let mean = blanks.iter().map(|&(r, c)| raw[&(r, c, t)]).sum::<f64>() / blanks.len() as f64;
        for (&(r, c, tt), &v) in raw.range((0, 0, t)..) {
            if tt != t {
                continue;
            }
            let e = exported[&(r, c, t)];
            assert!(
                (v - mean - e).abs() <= EXPORT_DIGITS,
                "well {r},{c} t {}: {} vs export {e}",
                f64::from_bits(t),
                v - mean
            );
            n += 1;
        }
    }
    assert_eq!(n, 53_664);
    eprintln!("{n} exported values = raw − mean(blank group)");
}

#[test]
fn cuvette_spectra_are_refused() {
    let doc = corpus_dir().join("softmax5-cuvette-spectra-s2.pda");
    if !doc.exists() {
        eprintln!("skip: missing");
        return;
    }
    match openreadout_plate::PlateReader.open(&doc) {
        Err(openreadout_core::Error::Unsupported { .. }) => {}
        Err(e) => panic!("expected an unsupported-feature refusal, got {e}"),
        Ok(_) => panic!("cuvette spectra must be refused, not decoded"),
    }
}

#[test]
fn smp7_absorbance_document_without_export() {
    let doc = corpus_dir().join("softmax7-elisa-m5.sda");
    if !doc.exists() {
        eprintln!("skip: missing");
        return;
    }
    let mut ds = openreadout_plate::PlateReader.open(&doc).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.tables.len(), 10);
    for (i, t) in info.tables.iter().enumerate() {
        assert_eq!(t.row_count, 96);
        // TMB read at 605 nm, then at 450 nm after the sulfuric-acid stop (plates 5-10)
        let wl = if i < 4 { 605.0 } else { 450.0 };
        assert_eq!(t.extra["reads"][0]["wavelength_nm"], wl);
        assert_eq!(
            t.extra["acquired_at"].as_str().map(|s| &s[..10]),
            Some("2020-09-06")
        );
        assert_eq!(t.extra["instrument"]["model"], "SpectraMax M5");
    }
    let tab = ds.read_table(0, 0, 96).unwrap();
    // A1 of plate `10min` as stored (0.7681)
    assert_eq!(tab.columns[6][0], 0.7681);
}

/// Half a unit of the last decimal the export prints for `v` (Gen5's Excel export rounds to
/// its number format, 3 decimals for OD; integers are exact).
fn export_tolerance(v: f64) -> f64 {
    let s = format!("{v}");
    let d = s.split_once('.').map_or(0, |(_, f)| f.len());
    0.5 * 10f64.powi(-i32::try_from(d).unwrap()) + 1e-9
}

/// (plate/well, read label, time bits) -> value of a decoded file.
fn labelled(path: &Path) -> BTreeMap<(String, String, u64), f64> {
    let mut ds = openreadout_plate::PlateReader.open(path).unwrap();
    let info = ds.info().unwrap();
    let mut out = BTreeMap::new();
    for t in &info.tables {
        let reads = t.extra["reads"].as_array().unwrap().clone();
        let tab = ds.read_table(t.index, 0, t.row_count).unwrap();
        for i in 0..tab.columns[0].len() {
            let v = tab.columns[6][i];
            if !v.is_finite() {
                continue;
            }
            let row = tab.columns[1][i] as u8 - 1;
            let well = format!("{}{}", char::from(b'A' + row), tab.columns[2][i]);
            let label = reads[tab.columns[3][i] as usize - 1]["label"]
                .as_str()
                .unwrap()
                .to_string();
            let time = tab.columns[5][i];
            let tb = if time.is_nan() {
                u64::MAX
            } else {
                time.to_bits()
            };
            let plate = t.name.clone().unwrap_or_default();
            out.insert((format!("{plate}/{well}"), label, tb), v);
        }
    }
    out
}

/// Gen5 experiment files against the depositor's exports (oracle/plate_exports.py): every
/// exported value is the decoded value of the same well, read and time within the export's
/// rounding; reader, serial number, software version, read time and temperature agree.
#[test]
fn gen5_experiments_match_their_exports() {
    let dir = root().join("corpus/oracle/plate-binary");
    let mut compared = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let o: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let id = o["id"].as_str().unwrap();
        let doc = corpus_dir().join(format!("{id}.xpt"));
        if !doc.exists() {
            eprintln!("skip {id}: missing");
            continue;
        }
        let ours = labelled(&doc);
        let mut n = 0;
        for v in o["values"].as_array().unwrap() {
            let (well, label, time, value) = (
                v[0].as_str().unwrap(),
                v[1].as_str().unwrap(),
                v[2].as_f64(),
                v[3].as_f64().unwrap(),
            );
            let tb = time.map_or(u64::MAX, f64::to_bits);
            let key = (format!("Plate 1/{well}"), label.to_string(), tb);
            let x = ours
                .get(&key)
                .unwrap_or_else(|| panic!("{id}: {key:?} not decoded"));
            assert!(
                (x - value).abs() <= export_tolerance(value),
                "{id} {key:?}: decoded {x}, export {value}"
            );
            n += 1;
        }
        let ds = openreadout_plate::PlateReader.open(&doc).unwrap();
        let info = ds.info().unwrap();
        let e = &info.tables[0].extra;
        let h = &o["header"];
        if let Some(m) = h["Reader Type"].as_str() {
            assert_eq!(e["instrument"]["model"], m, "{id}: model");
        }
        if let Some(s) = h["Reader Serial Number"].as_str() {
            assert_eq!(e["instrument"]["serial_number"], s, "{id}: serial");
        }
        if let Some(v) = h["Software Version"].as_str() {
            assert_eq!(e["software_version"], v, "{id}: version");
        }
        if let (Some(d), Some(t)) = (h["Date"].as_str(), h["Time"].as_str()) {
            // the export prints the plate's time (stored in UTC; local = UTC + a whole number
            // of hours); `acquired_at` is the reads' own (local) time, within a minute of it
            let secs = |s: &str| -> i64 {
                let (day, clock) = s.split_once('T').unwrap();
                let dn: i64 = day.replace('-', "").parse().unwrap();
                let c: Vec<i64> = clock
                    .trim_end_matches('Z')
                    .split(':')
                    .map(|x| x.parse().unwrap())
                    .collect();
                (dn % 100) * 86_400 + c[0] * 3600 + c[1] * 60 + c[2]
            };
            let export = secs(&format!("{d}T{t}"));
            let ours = secs(e["acquired_at"].as_str().unwrap());
            assert!(
                (ours - export).abs() <= 60,
                "{id}: read time {ours} vs export {export}"
            );
            let utc = secs(e["plate_time_utc"].as_str().unwrap());
            assert_eq!(
                (export - utc).rem_euclid(3600),
                0,
                "{id}: plate time is the export's"
            );
        }
        if let Some(t) = h["Actual Temperature"].as_f64() {
            assert_eq!(e["temperature_c"].as_f64(), Some(t), "{id}: temperature");
        }
        eprintln!("{id}: {n} exported values equal the decoded ones");
        compared += n;
    }
    eprintln!("{compared} Gen5 export values compared");
}

#[test]
fn gen5_protocols_and_headerless_blocks_are_not_guessed() {
    let prt = corpus_dir().join("gen5prt-elisa-reader.prt");
    if prt.exists() {
        match openreadout_plate::PlateReader.open(&prt) {
            Err(openreadout_core::Error::Unsupported { .. }) => {}
            Err(e) => panic!("expected an unsupported-feature refusal, got {e}"),
            Ok(_) => panic!("a Gen5 protocol holds no data and must be refused"),
        }
    }
    let fret = corpus_dir().join("gen5xpt-synergyh1-fl-endpoint.xpt");
    if fret.exists() {
        let mut ds = openreadout_plate::PlateReader.open(&fret).unwrap();
        let info = ds.info().unwrap();
        // plates 2 and 3 have three reads; plate 1 lost the header of its first read
        let reads: Vec<usize> = info
            .tables
            .iter()
            .map(|t| t.extra["reads"].as_array().unwrap().len())
            .collect();
        assert_eq!(reads, vec![2, 3, 3]);
        let report = ds.check().unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "read_not_decoded" && f.message.contains("no read header")),
            "the orphan record block is reported"
        );
    }
}
