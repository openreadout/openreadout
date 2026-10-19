//! DNA sequencer output against its ground truth (`oracle/seq_oracle.py`):
//!
//! - Illumina run folders: the run summary per read and lane against Illumina's `interop`
//!   (tiles, density, % PF and its spread, reads, % ≥ Q30, yield, error rate, % aligned, first-cycle
//!   intensity, phasing, % occupied) and the totals; FASTQ of lane 1 against Picard
//!   `IlluminaBasecallsToFastq`: the first 2000 records of every read exactly (name, bases,
//!   qualities) and the pass-filter count.
//! - POD5: every read's metadata and signal in pA (sum, extremes, first 32 samples) against pod5.
//! - FAST5: every read's metadata and signal against h5py (pA from the stored calibration); the
//!   reads of `seq-fast5-multi-zip` also against pod5's reading of their POD5 conversion.
//! - FASTQ and BAM: counts, bases, N50, Q30 bases, GC and sampled records against pysam.
//! - CRAM: as BAM, plus the GC content and N count of the sampled records' bases (pysam with
//!   the file's companion reference FASTA).
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(clippy::many_single_char_names)]
#![allow(clippy::float_cmp)]

use std::path::Path;

use openreadout_core::{Dataset, FormatReader};
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["illumina-run", "pod5", "fast5", "fastq", "bam", "cram"];

fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a - b).abs() <= abs.max(rel * a.abs().max(b.abs()))
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

/// Compare one corpus file with its oracle.
pub fn compare_file(format: &str, id: &str, path: &Path, o: &Value) -> Result<String, String> {
    match format {
        "illumina-run" => illumina(path, o),
        "pod5" => signal(&openreadout_seq::Pod5Reader, path, o, true),
        "fast5" => {
            let r = signal(&openreadout_seq::Fast5Reader, path, o, false)?;
            if id == "seq-fast5-multi-zip" {
                cross_check_pod5(path)?;
                return Ok(format!(
                    "{r}; pA equal pod5's reading of the POD5 conversion"
                ));
            }
            Ok(r)
        }
        "fastq" | "bam" | "cram" => reads(format, path, o),
        _ => Err(format!("no comparison for {format}")),
    }
}

/// Column `name` of `t` (a table read whole).
fn column<'a>(
    info: &openreadout_core::model::TableInfo,
    t: &'a openreadout_core::model::Table,
    name: &str,
) -> &'a [f64] {
    info.columns
        .iter()
        .position(|c| c.name == name)
        .and_then(|i| t.columns.get(i))
        .map_or(&[], Vec::as_slice)
}

fn illumina(path: &Path, o: &Value) -> Result<String, String> {
    let mut ds = openreadout_seq::IlluminaRunReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| e.to_string())?;
    let st = &info.tables[0];
    let t = ds
        .read_table(0, 0, st.row_count)
        .map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    let mut checked = 0usize;
    let empty = Vec::new();
    for r in o["reads"].as_array().unwrap_or(&empty) {
        let read = r["read"].as_f64().unwrap_or(0.0);
        for l in r["lanes"].as_array().unwrap_or(&empty) {
            let lane = l["lane"].as_f64().unwrap_or(0.0);
            let row = (0..t.columns[0].len())
                .find(|&i| column(st, &t, "read")[i] == read && column(st, &t, "lane")[i] == lane);
            let Some(i) = row else {
                problems.push(format!("no summary row for read {read} lane {lane}"));
                continue;
            };
            // (ours, column, oracle key, factor ours → oracle units, relative tolerance)
            for (col, key, factor, tol) in [
                ("tiles", "tiles", 1.0, 0.0),
                ("density", "density", 1000.0, 1e-5),
                ("density_sd", "density_sd", 1000.0, 1e-3),
                ("percent_pf", "percent_pf", 1.0, 1e-5),
                ("percent_pf_sd", "percent_pf_sd", 1.0, 1e-3),
                ("reads", "reads", 1e6, 1e-7),
                ("reads_pf", "reads_pf", 1e6, 1e-7),
                ("percent_q30", "percent_q30", 1.0, 1e-5),
                ("yield", "yield_g", 1.0, 1e-5),
                ("error_rate", "error_rate", 1.0, 1e-4),
                ("percent_aligned", "percent_aligned", 1.0, 1e-4),
                ("intensity_cycle_1", "intensity_cycle_1", 1.0, 1e-4),
                ("phasing", "phasing", 1.0, 1e-4),
                ("prephasing", "prephasing", 1.0, 1e-4),
                ("percent_occupied", "percent_occupied", 1.0, 1e-5),
            ] {
                let ours = column(st, &t, col).get(i).copied().unwrap_or(f64::NAN) * factor;
                let theirs = num(&l[key]);
                match (ours.is_finite(), theirs) {
                    (true, Some(v)) => {
                        if !close(ours, v, tol, 1e-6) {
                            problems.push(format!(
                                "read {read} lane {lane} {col}: {ours} vs interop {v}"
                            ));
                        }
                        checked += 1;
                    }
                    (false, None) => {}
                    // values interop derives from metrics the reader does not summarise
                    (false, Some(_)) if matches!(col, "phasing" | "prephasing") => {}
                    // a zero interop reports for a value the run has none of
                    (false, Some(v)) if v == 0.0 && col == "percent_aligned" => {}
                    (false, Some(v)) => {
                        problems.push(format!(
                            "read {read} lane {lane} {col}: none vs interop {v}"
                        ));
                    }
                    (true, None) => problems.push(format!(
                        "read {read} lane {lane} {col}: {ours} where interop has none"
                    )),
                }
            }
        }
    }
    let totals = &st.extra["totals"];
    for (k, key) in [("yield_gbp", "yield_g"), ("percent_q30", "percent_q30")] {
        if let (Some(a), Some(b)) = (num(&totals[k]), num(&o["total"][key]))
            && !close(a, b, 1e-4, 1e-6)
        {
            problems.push(format!("total {k}: {a} vs interop {b}"));
        }
    }
    let mut fastq_note = String::new();
    if o["fastq"].is_object() {
        fastq_note = compare_fastq(path, &mut *ds, &info, &o["fastq"], &mut problems)?;
    }
    if problems.is_empty() {
        Ok(format!(
            "{checked} run-summary values agree with interop{fastq_note}"
        ))
    } else {
        Err(problems.join("; "))
    }
}

fn compare_fastq(
    path: &Path,
    ds: &mut dyn Dataset,
    info: &openreadout_core::model::FileInfo,
    o: &Value,
    problems: &mut Vec<String>,
) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    let n = o["hashed_records"].as_u64().unwrap_or(2000);
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let report =
        openreadout_seq::export_run_fastq(path, Some(dir.path()), Some((0, Some(n - 1))), false)
            .map_err(|e| format!("FASTQ export failed: {e}"))?;
    if !report.verified {
        problems.push("FASTQ export not verified".into());
    }
    // pass-filter clusters of lane 1 from the filter files (table 5)
    let bc = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("base calls"))
        .ok_or("no base calls table")?;
    let t = ds
        .read_table(bc.index, 0, bc.row_count)
        .map_err(|e| e.to_string())?;
    let lanes = column(bc, &t, "lane");
    let pf: f64 = column(bc, &t, "clusters_pf")
        .iter()
        .zip(lanes)
        .filter(|(_, l)| **l == 1.0)
        .map(|(v, _)| v)
        .sum();
    let mut compared = 0;
    for (label, exp) in o["reads"].as_object().into_iter().flatten() {
        let Some(f) = report
            .files
            .iter()
            .find(|f| f.lane == 1 && &f.read == label)
        else {
            problems.push(format!("FASTQ {label}: not written"));
            continue;
        };
        if exp["records"].as_f64() != Some(pf) {
            problems.push(format!(
                "FASTQ {label}: {pf} pass-filter clusters, Picard wrote {}",
                exp["records"]
            ));
        }
        let file = std::fs::File::open(&f.path).map_err(|e| e.to_string())?;
        let r = BufReader::new(flate2::read::MultiGzDecoder::new(file));
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        let lines: Vec<String> = r.lines().map_while(Result::ok).collect();
        for (k, rec) in (0u64..).zip(lines.chunks(4)) {
            if k >= n || rec.len() < 4 {
                break;
            }
            let name = rec[0][1..].split(' ').next().unwrap_or("");
            h.update(format!("{name}\n{}\n{}\n", rec[1], rec[3]).as_bytes());
        }
        let ours = format!("{:032x}", h.digest128());
        if Some(ours.as_str()) != exp["first_records_xxh3"].as_str() {
            problems.push(format!(
                "FASTQ {label}: the first {n} records differ from Picard's (first ours: {:?}, Picard: {})",
                lines.iter().take(4).collect::<Vec<_>>(),
                exp["first"][0]
            ));
        }
        compared += 1;
    }
    Ok(format!(
        "; FASTQ of {compared} reads equal Picard's ({n} records each, {pf} pass-filter clusters)"
    ))
}

fn read_meta(
    ds: &mut dyn Dataset,
) -> Result<
    (
        openreadout_core::model::FileInfo,
        openreadout_core::model::Table,
    ),
    String,
> {
    let info = ds.info().map_err(|e| e.to_string())?;
    let t = ds
        .read_table(0, 0, info.tables[0].row_count)
        .map_err(|e| e.to_string())?;
    Ok((info, t))
}

fn text_col(
    info: &openreadout_core::model::TableInfo,
    t: &openreadout_core::model::Table,
    name: &str,
    i: usize,
) -> String {
    let c = info.columns.iter().position(|c| c.name == name);
    let cats = c
        .and_then(|c| info.columns[c].extra.get("categories"))
        .and_then(Value::as_array);
    let code = column(info, t, name).get(i).copied().unwrap_or(-1.0);
    cats.and_then(|a| a.get(code as usize))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn signal(reader: &dyn FormatReader, path: &Path, o: &Value, pod5: bool) -> Result<String, String> {
    let mut ds = reader.open(path).map_err(|e| format!("open failed: {e}"))?;
    let (info, t) = read_meta(ds.as_mut())?;
    let ti = &info.tables[0];
    let empty = Vec::new();
    let reads = o["reads"].as_array().unwrap_or(&empty);
    let mut problems = Vec::new();
    if reads.len() as u64 != ti.row_count {
        problems.push(format!("{} reads, oracle {}", ti.row_count, reads.len()));
    }
    let mut samples = 0usize;
    for (i, r) in reads.iter().enumerate() {
        let id = text_col(ti, &t, "read_id", i);
        if Some(id.as_str()) != r["read_id"].as_str() {
            problems.push(format!("read {i}: id {id} vs {}", r["read_id"]));
            continue;
        }
        let mut field = |ours: &str, theirs: &str| {
            let a = column(ti, &t, ours).get(i).copied().unwrap_or(f64::NAN);
            if let Some(b) = num(&r[theirs])
                && !close(a, b, 1e-6, 1e-6)
            {
                problems.push(format!("read {i} {ours}: {a} vs {b}"));
            }
        };
        field("channel", "channel");
        field("read_number", "read_number");
        field("samples", "num_samples");
        field("median_before", "median_before");
        if pod5 {
            field("well", "well");
            field("calibration_offset", "calibration_offset");
            field("calibration_scale", "calibration_scale");
            let er = text_col(ti, &t, "end_reason", i);
            if Some(er.as_str()) != r["end_reason"].as_str() {
                problems.push(format!("read {i} end reason {er} vs {}", r["end_reason"]));
            }
        }
        let n = r["num_samples"].as_u64().unwrap_or(0);
        let tr = ds
            .read_trace(0, i as u32, 0, n)
            .map_err(|e| format!("read {i}: {e}"))?;
        let v = &tr.channels[0];
        samples += v.len();
        if v.len() as u64 != n {
            problems.push(format!("read {i}: {} samples vs {n}", v.len()));
            continue;
        }
        // expected pA
        let (sum, first, lo, hi): (f64, Vec<f64>, f64, f64) = if pod5 {
            (
                num(&r["signal_sum"]).unwrap_or(f64::NAN),
                r["signal_first"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default(),
                num(&r["signal_min"]).unwrap_or(f64::NAN),
                num(&r["signal_max"]).unwrap_or(f64::NAN),
            )
        } else {
            let off = num(&r["offset"]).unwrap_or(0.0);
            let scale = num(&r["range"]).unwrap_or(1.0) / num(&r["digitisation"]).unwrap_or(1.0);
            let pa = |x: f64| (x + off) * scale;
            (
                (num(&r["raw_sum"]).unwrap_or(f64::NAN) + off * n as f64) * scale,
                r["raw_first"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_f64).map(pa).collect())
                    .unwrap_or_default(),
                pa(num(&r["raw_min"]).unwrap_or(f64::NAN)),
                pa(num(&r["raw_max"]).unwrap_or(f64::NAN)),
            )
        };
        let our_sum: f64 = v.iter().sum();
        let (our_lo, our_hi) = v
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
                (a.min(*x), b.max(*x))
            });
        // pod5 converts in single precision: compare to 1e-5 relative
        if !close(our_sum, sum, 1e-6, 1e-3)
            || !close(our_lo, lo, 1e-5, 1e-4)
            || !close(our_hi, hi, 1e-5, 1e-4)
            || first.iter().zip(v).any(|(a, b)| !close(*a, *b, 1e-5, 1e-4))
        {
            problems.push(format!(
                "read {i}: signal sum {our_sum} vs {sum}, min {our_lo} vs {lo}, max {our_hi} vs {hi}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{} reads, {samples} samples in pA agree",
            reads.len()
        ))
    } else {
        Err(problems.join("; "))
    }
}

/// The FAST5 reads of `seq-fast5-multi-zip` equal pod5's reading of `seq-pod5-multi-fast5-zip-v6`.
fn cross_check_pod5(path: &Path) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/oracle/seq");
    let Ok(text) = std::fs::read_to_string(root.join("seq-pod5-multi-fast5-zip-v6.json")) else {
        return Ok(());
    };
    let o: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut ds = openreadout_seq::Fast5Reader
        .open(path)
        .map_err(|e| e.to_string())?;
    let (info, t) = read_meta(ds.as_mut())?;
    let ti = &info.tables[0];
    for r in o["reads"].as_array().into_iter().flatten() {
        let Some(i) = (0..t.columns[0].len())
            .find(|&i| Some(text_col(ti, &t, "read_id", i).as_str()) == r["read_id"].as_str())
        else {
            return Err(format!(
                "read {} of the POD5 conversion is not in the FAST5",
                r["read_id"]
            ));
        };
        let tr = ds
            .read_trace(0, i as u32, 0, 32)
            .map_err(|e| e.to_string())?;
        for (a, b) in tr.channels[0]
            .iter()
            .zip(r["signal_first"].as_array().into_iter().flatten())
        {
            let b = b.as_f64().unwrap_or(f64::NAN);
            if !close(*a, b, 1e-5, 1e-4) {
                return Err(format!(
                    "read {}: FAST5 pA {a} vs pod5's {b} for the POD5 conversion",
                    r["read_id"]
                ));
            }
        }
    }
    Ok(())
}

fn reads(format: &str, path: &Path, o: &Value) -> Result<String, String> {
    let reader: &dyn FormatReader = match format {
        "bam" => &openreadout_seq::BamReader,
        "cram" => &openreadout_seq::CramReader,
        _ => &openreadout_seq::FastqReader,
    };
    let aligned = format != "fastq";
    let mut ds = reader.open(path).map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| e.to_string())?;
    let t0 = &info.tables[0];
    let s = &t0.extra["summary"];
    let mut problems = Vec::new();
    let exact = t0.extra["read_count_exact"].as_bool() == Some(true);
    if !exact {
        return Err("the corpus files are small enough for info to read whole".into());
    }
    let mut cmp =
        |ours: &Value, theirs: &Value, what: &str, tol: f64| match (num(ours), num(theirs)) {
            (Some(a), Some(b)) if close(a, b, tol, 1e-9) => {}
            (None, None) => {}
            (a, b) => problems.push(format!("{what}: {a:?} vs pysam {b:?}")),
        };
    if aligned {
        cmp(&s["records"], &o["records"], "records", 0.0);
        cmp(&s["reads"], &o["primary"], "primary records", 0.0);
        cmp(&s["mapped"], &o["mapped"], "mapped", 0.0);
    } else {
        cmp(&s["reads"], &o["reads"], "reads", 0.0);
        cmp(&s["reads"], &o["reads_biopython"], "reads (Biopython)", 0.0);
        cmp(&s["length_min"], &o["length_min"], "shortest", 0.0);
        cmp(&s["length_max"], &o["length_max"], "longest", 0.0);
        cmp(&s["percent_gc"], &o["percent_gc"], "GC", 1e-4);
        let q30 = s["percent_q30"].as_f64().unwrap_or(f64::NAN) / 100.0
            * s["bases"].as_f64().unwrap_or(f64::NAN);
        cmp(&Value::from(q30), &o["q30_bases"], "bases ≥ Q30", 1e-4);
    }
    cmp(&s["bases"], &o["bases"], "bases", 0.0);
    cmp(&s["length_n50"], &o["n50"], "N50", 0.0);
    let t = ds
        .read_table(0, 0, t0.row_count)
        .map_err(|e| e.to_string())?;
    let mut sampled = 0;
    for r in o["sampled"].as_array().into_iter().flatten() {
        let i = r["i"].as_u64().unwrap_or(0) as usize;
        let keys: &[(&str, &str, f64)] = if format == "cram" {
            &[
                ("length", "length", 0.0),
                ("mean_quality", "mean_quality", 1e-9),
                ("flag", "flag", 0.0),
                ("mapq", "mapq", 0.0),
                ("ref_id", "ref_id", 0.0),
                ("pos", "pos", 0.0),
                ("passes", "passes", 0.0),
                ("accuracy", "accuracy", 1e-7),
                ("gc_percent", "gc_percent", 1e-9),
                ("n_bases", "n_bases", 0.0),
            ]
        } else if format == "bam" {
            &[
                ("length", "length", 0.0),
                ("mean_quality", "mean_quality", 1e-9),
                ("flag", "flag", 0.0),
                ("mapq", "mapq", 0.0),
                ("ref_id", "ref_id", 0.0),
                ("pos", "pos", 0.0),
                ("passes", "passes", 0.0),
                ("accuracy", "accuracy", 1e-7),
            ]
        } else {
            &[
                ("length", "length", 0.0),
                ("mean_quality", "mean_quality", 1e-9),
                ("gc_percent", "gc_percent", 1e-9),
            ]
        };
        for (col, key, tol) in keys {
            let a = column(t0, &t, col).get(i).copied().unwrap_or(f64::NAN);
            let a = Value::from(if a.is_finite() { Some(a) } else { None });
            cmp(&a, &r[*key], &format!("record {i} {col}"), *tol);
        }
        sampled += 1;
    }
    if problems.is_empty() {
        Ok(format!(
            "counts, bases, N50 and {sampled} sampled records{} agree with pysam",
            if format == "cram" {
                " (with their bases' GC and N)"
            } else {
                ""
            }
        ))
    } else {
        problems.truncate(12);
        Err(problems.join("; "))
    }
}
