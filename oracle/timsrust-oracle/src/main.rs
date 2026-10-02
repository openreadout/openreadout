//! Ground truth for timsTOF `.d` corpus files.
//!
//! Usage: `timsrust-oracle <dataset.d> <id> <out.json>` — run it on a COPY of the dataset:
//! timsrust opens the SQLite database through rusqlite, which may checkpoint a write-ahead log.
//!
//! Frames (scan offsets, TOF indices, intensities) and the m/z and 1/K0 conversions come from
//! timsrust 0.6.6, run as a black box. Tables come from SQLite itself (rusqlite). The spectra
//! OpenReadout defines (docs/formats/bruker-tdf.md: MS1 frames summed over scans, DDA-PASEF
//! precursors summed over their selections, DIA-PASEF windows) are assembled here from those
//! independent inputs, in the corpus harness's `spectra` oracle format.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use timsrust::TimsTofPath;
use timsrust::core::{Converter, ScanIndex};
use xxhash_rust::xxh3::xxh3_128;

fn hex(b: &[u8]) -> String {
    format!("{:032x}", xxh3_128(b))
}

fn has_table(c: &Connection, t: &str) -> bool {
    c.query_row(
        "select count(*) from sqlite_master where type='table' and lower(name)=lower(?1)",
        [t],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

struct Sel {
    frame: i64,
    begin: usize,
    end: usize,
    iso_mz: f64,
    iso_w: f64,
    ce: f64,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, id, out) = (&args[1], &args[2], &args[3]);
    let tims = TimsTofPath::new(path).expect("timsrust opens the dataset");
    let reader = tims.frame_reader().expect("frame reader");
    let mzc = tims.mz_converter().expect("mz converter");
    let imc = tims.im_converter();
    let db = Connection::open_with_flags(
        format!("{path}/analysis.tdf"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("sqlite");

    // Frames table
    let mut stmt = db
        .prepare("select Id, Time, Polarity, MsMsType, NumScans, NumPeaks from Frames order by Id")
        .unwrap();
    let frames: Vec<(i64, f64, String, i64, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2).unwrap_or_default(), r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();

    // every frame through timsrust
    let mut ions: BTreeMap<i64, (Vec<usize>, Vec<u32>, Vec<u32>)> = BTreeMap::new();
    let mut frame_json = Vec::new();
    let mut empty_frames = Vec::new();
    for f in &frames {
        // timsrust reports a blob without payload ("No binary data") as an error; such a frame
        // holds no peaks, so it is recorded as NumScans empty scans and listed in `empty_frames`.
        let (offs, tofs, ints, rt) = match reader.get_frame(f.0 as usize) {
            Ok(fr) => {
                let ion = fr.ions();
                (
                    ion.scan_offsets().clone(),
                    ion.tof_indices().iter().map(|&t| u32::from(t)).collect::<Vec<u32>>(),
                    ion.intensities().iter().map(|&i| u32::from(i)).collect::<Vec<u32>>(),
                    fr.info().rt_in_seconds(),
                )
            }
            Err(e) if format!("{e:?}").contains("No binary data") => {
                empty_frames.push(f.0);
                (vec![0usize; f.4 as usize + 1], Vec::new(), Vec::new(), f.1)
            }
            Err(e) => panic!("timsrust frame {}: {e:?}", f.0),
        };
        let ob: Vec<u8> = offs.iter().flat_map(|&o| (o as u64).to_le_bytes()).collect();
        let tb: Vec<u8> = tofs.iter().flat_map(|t| t.to_le_bytes()).collect();
        let ib: Vec<u8> = ints.iter().flat_map(|t| t.to_le_bytes()).collect();
        frame_json.push(json!({
            "id": f.0, "scans": offs.len().saturating_sub(1), "peaks": tofs.len(),
            "rt_s": rt,
            "xxh3_scan_offsets": hex(&ob), "xxh3_tof": hex(&tb), "xxh3_intensity": hex(&ib),
        }));
        ions.insert(f.0, (offs, tofs, ints));
    }

    // DDA-PASEF
    let mut precursors: BTreeMap<i64, (Option<f64>, Option<f64>, Option<i64>, Option<f64>, Option<f64>)> = BTreeMap::new();
    if has_table(&db, "Precursors") {
        let mut s = db.prepare("select Id, MonoisotopicMz, LargestPeakMz, Charge, ScanNumber, Intensity from Precursors").unwrap();
        for r in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))).unwrap() {
            let (k, v) = r.unwrap();
            precursors.insert(k, v);
        }
    }
    let mut sels: BTreeMap<i64, Vec<Sel>> = BTreeMap::new();
    if has_table(&db, "PasefFrameMsMsInfo") {
        let mut s = db.prepare("select Frame, ScanNumBegin, ScanNumEnd, IsolationMz, IsolationWidth, CollisionEnergy, Precursor from PasefFrameMsMsInfo order by Frame, ScanNumBegin").unwrap();
        for r in s.query_map([], |r| {
            Ok((r.get::<_, i64>(6)?, Sel { frame: r.get(0)?, begin: r.get::<_, i64>(1)? as usize, end: r.get::<_, i64>(2)? as usize, iso_mz: r.get(3)?, iso_w: r.get(4)?, ce: r.get(5)? }))
        }).unwrap() {
            let (p, s) = r.unwrap();
            if precursors.contains_key(&p) {
                sels.entry(p).or_default().push(s);
            }
        }
    }
    // DIA-PASEF
    let mut dia_group: BTreeMap<i64, i64> = BTreeMap::new();
    let mut windows: BTreeMap<i64, Vec<Sel>> = BTreeMap::new();
    if has_table(&db, "DiaFrameMsMsInfo") {
        let mut s = db.prepare("select Frame, WindowGroup from DiaFrameMsMsInfo").unwrap();
        for r in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).unwrap() {
            let (f, g) = r.unwrap();
            dia_group.insert(f, g);
        }
        let mut s = db.prepare("select WindowGroup, ScanNumBegin, ScanNumEnd, IsolationMz, IsolationWidth, CollisionEnergy from DiaFrameMsMsWindows order by WindowGroup, ScanNumBegin").unwrap();
        for r in s.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, Sel { frame: 0, begin: r.get::<_, i64>(1)? as usize, end: r.get::<_, i64>(2)? as usize, iso_mz: r.get(3)?, iso_w: r.get(4)?, ce: r.get(5)? }))
        }).unwrap() {
            let (g, s) = r.unwrap();
            windows.entry(g).or_default().push(s);
        }
    }

    // spectra in our order
    let mut first_frame: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for (p, ss) in &sels {
        if let Some(f) = ss.iter().map(|s| s.frame).min() {
            first_frame.entry(f).or_default().push(*p);
        }
    }
    let summed = |parts: Vec<(i64, usize, usize)>| -> (Vec<u32>, Vec<u64>) {
        let mut acc: BTreeMap<u32, u64> = BTreeMap::new();
        for (f, b, e) in parts {
            let (offs, tofs, ints) = &ions[&f];
            let n = offs.len() - 1;
            let (b, e) = (b.min(n), e.min(n).max(b.min(n)));
            for k in offs[b]..offs[e] {
                *acc.entry(tofs[k]).or_default() += u64::from(ints[k]);
            }
        }
        acc.into_iter().unzip()
    };
    let mut scans = Vec::new();
    let mut push = |spec: Value, parts: Vec<(i64, usize, usize)>, scans: &mut Vec<Value>| {
        let (t, s) = summed(parts);
        let mz: Vec<f64> = t
            .iter()
            .map(|&x| f64::from(mzc.convert(timsrust::core::TofIndex::try_from(x).unwrap())))
            .collect();
        let it: Vec<f32> = s.iter().map(|&v| v as f32).collect();
        let mzb: Vec<u8> = mz.iter().flat_map(|v| v.to_le_bytes()).collect();
        let ib: Vec<u8> = it.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut o = spec.as_object().unwrap().clone();
        let i = scans.len();
        o.insert("index".into(), json!(i));
        o.insert("scan_number".into(), json!(i + 1));
        o.insert("centroided".into(), json!(false));
        o.insert("filter".into(), Value::Null);
        o.insert("n_peaks".into(), json!(mz.len()));
        o.insert("mz_bits".into(), Value::Null);
        o.insert("xxh3_mz".into(), json!(hex(&mzb)));
        o.insert("xxh3_intensity".into(), json!(hex(&ib)));
        o.insert("sum_intensity".into(), json!(it.iter().map(|&v| f64::from(v)).sum::<f64>()));
        o.insert("mz_min".into(), json!(mz.first()));
        o.insert("mz_max".into(), json!(mz.last()));
        o.insert("first_peaks".into(), json!(mz.iter().zip(&it).take(5).map(|(a, b)| [*a, f64::from(*b)]).collect::<Vec<_>>()));
        scans.push(Value::Object(o));
    };
    let pol = |p: &str| match p { "+" => "positive", "-" => "negative", _ => "unknown" };
    let rt_of: BTreeMap<i64, (f64, String)> = frames.iter().map(|f| (f.0, (f.1, f.2.clone()))).collect();
    let mut precursor_mobility = BTreeMap::new();
    for f in &frames {
        let (fid, rt, p, msms, nscans, _) = f;
        if *msms == 8 {
            for pid in first_frame.get(fid).cloned().unwrap_or_default() {
                let (mono, largest, charge, scan, _int) = precursors[&pid];
                let ss = &sels[&pid];
                let first = ss.iter().map(|s| s.frame).min().unwrap();
                let (rt1, p1) = rt_of[&first].clone();
                if let (Some(sc), Some(imc)) = (scan, imc.as_ref()) {
                    let im = f64::from(imc.convert(ScanIndex::try_from(sc as u32).unwrap()));
                    precursor_mobility.insert(pid, im);
                }
                push(json!({
                    "ms_level": 2, "rt_s": rt1, "polarity": pol(&p1),
                    "precursor_mz": mono.or(largest), "precursor_charge": charge,
                    "activation": Value::Null,
                    "isolation_window_mz": [ss[0].iso_mz - ss[0].iso_w / 2.0, ss[0].iso_mz + ss[0].iso_w / 2.0],
                    "collision_energy": ss[0].ce,
                    "inverse_reduced_mobility": precursor_mobility.get(&pid),
                }), ss.iter().map(|s| (s.frame, s.begin, s.end)).collect(), &mut scans);
            }
        } else if *msms == 9 {
            let ws = dia_group.get(fid).and_then(|g| windows.get(g));
            for w in ws.map(|v| v.as_slice()).unwrap_or(&[]) {
                push(json!({
                    "ms_level": 2, "rt_s": rt, "polarity": pol(p), "precursor_mz": Value::Null,
                    "precursor_charge": Value::Null, "activation": Value::Null,
                    "isolation_window_mz": [w.iso_mz - w.iso_w / 2.0, w.iso_mz + w.iso_w / 2.0],
                    "collision_energy": w.ce,
                }), vec![(*fid, w.begin, w.end)], &mut scans);
            }
        } else {
            push(json!({
                "ms_level": if *msms == 0 { 1 } else { 2 }, "rt_s": rt, "polarity": pol(p),
                "precursor_mz": Value::Null, "precursor_charge": Value::Null, "activation": Value::Null,
            }), vec![(*fid, 0, *nscans as usize)], &mut scans);
        }
    }
    let mz_samples: Vec<Value> = [0u32, 1, 1000, 100_000, 300_000]
        .iter()
        .map(|&t| json!([t, f64::from(mzc.convert(timsrust::core::TofIndex::try_from(t).unwrap()))]))
        .collect();
    let im_samples: Vec<Value> = imc
        .as_ref()
        .map(|c| [0u32, 1, 100, 500].iter().map(|&s| json!([s, f64::from(c.convert(ScanIndex::try_from(s).unwrap()))])).collect())
        .unwrap_or_default();
    let _ = BTreeSet::<i64>::new();
    let doc = json!({
        "id": id,
        "reader": "timsrust 0.6.6 (frames, m/z and 1/K0 converters) + rusqlite (tables); spectra assembled by oracle/timsrust-oracle",
        "images": [],
        "tdf": {"frames": frame_json, "empty_frames": empty_frames, "mz_samples": mz_samples, "mobility_samples": im_samples},
        "spectra": {"scan_count": scans.len(), "by_index": true, "scans": scans},
    });
    let body = doc["spectra"]["scans"].as_array().unwrap().iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",\n");
    let mut head = doc.clone();
    head["spectra"]["scans"] = json!([]);
    let text = serde_json::to_string_pretty(&head).unwrap();
    let text = text.replacen("\"scans\": []", &format!("\"scans\": [\n{body}\n]"), 1);
    serde_json::from_str::<Value>(&text).expect("valid JSON");
    std::fs::write(out, text).unwrap();
    println!("wrote {out}: {} frames, {} spectra", frames.len(), scans.len());
}
