//! Vendor-agreement benchmark of automatic peak integration: `openreadout analyze peaks` in every
//! baseline mode over every chromatogram of the development corpus that a vendor data system
//! integrated (Agilent ChemStation `Report.TXT` / `Result.xml` / ANDI peak tables, Shimadzu
//! LabSolutions ANDI peak tables, MSD ChemStation `RESULTS.CSV`, Agilent OpenLab CDS `.rx`
//! result packages, Thermo Chromeleon's stored results in `.cmbx` archives — read by our reader,
//! confirmed against Chromeleon's PDF report and by recomputation, `tests/chromeleon.rs`),
//! reported per vendor data set: peak recall, retention-time agreement, area
//! and area-% error distributions. Asserts that the default mode does not regress any vendor
//! against the fixed drop-line baseline and is closer to the vendors overall.
//!
//! Ground truth: `corpus/oracle/quant/*.json` (`oracle/quant.py`) and
//! `corpus/oracle/openlab/*.json` (`oracle/openlab_cds.py`), the vendors' own peak tables read
//! with third-party tools. Held-out files are never used (`docs/benchmark/heldout.md`). Numbers
//! are in `book/src/guides/quantitation.md` → Validation.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test peak_agreement -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters
//! ids; `PEAK_AGREEMENT_DUMP=<dir>` writes every chromatogram with the vendor's and our peaks as
//! JSON (for plots); `PEAK_AGREEMENT_WORST=N` lists the N largest area errors per mode.
#![cfg(feature = "corpus")]
#![allow(clippy::float_cmp, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Registry};
use openreadout_quant::extract::{ChromRequest, Chromatogram, Target, extract};
use openreadout_quant::peaks::{AreaTimeUnit, BaselineMode, Peak, PeakParams, find_peaks};
use serde_json::{Value, json};

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

fn only(id: &str) -> bool {
    std::env::var("CORPUS_ONLY").map_or(true, |o| id.contains(&o))
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_chrom::OpenLabReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
}

/// Chromeleon archives with stored results, and their vendor data set.
const CHROMELEON: [(&str, &str); 9] = [
    (
        "cmbx-lauterbach-invivo-cascade",
        "Chromeleon GC-FID (stored results)",
    ),
    ("cmbx-lim-tsoye-48h", "Chromeleon GC-FID (stored results)"),
    (
        "cmbx-lim-carvone-standards",
        "Chromeleon GC-FID (stored results)",
    ),
    (
        "cmbx-lim-levodione-calibration",
        "Chromeleon GC-FID (stored results)",
    ),
    (
        "cmbx-lim-r-carvone-reactions",
        "Chromeleon GC-FID (stored results)",
    ),
    (
        "cmbx-figshare-milks-sugars",
        "Chromeleon HPAEC-PAD (stored results)",
    ),
    (
        "cmbx-figshare-sugars-qev",
        "Chromeleon HPAEC-PAD (stored results)",
    ),
    (
        "cmbx-flavokawain-skrining-20221109",
        "Chromeleon UPLC-DAD (stored results)",
    ),
    (
        "cmbx-textiles-2019-088",
        "Chromeleon UHPLC-PDA/MS (stored results)",
    ),
];

/// Chromeleon's stored peaks of every chromatogram, from the `vendor_peaks` table.
fn chromeleon_cases(out: &mut Vec<Case>) {
    for (id, fam) in CHROMELEON {
        if !only(id) {
            continue;
        }
        let path = corpus_dir().join(format!("chromeleon/{id}.cmbx"));
        if !path.exists() {
            continue;
        }
        let (_, mut ds) = registry().open(&path).unwrap();
        let info = ds.info().unwrap();
        let Some(t) = info
            .tables
            .iter()
            .find(|t| t.name.as_deref() == Some("vendor_peaks"))
            .cloned()
        else {
            continue;
        };
        let tab = ds.read_table(t.index, 0, t.row_count).unwrap();
        let col = |n: &str| &tab.columns[t.columns.iter().position(|c| c.name == n).unwrap()];
        let (tr, rt, a, s0, s1) = (
            col("trace"),
            col("rt_min"),
            col("area"),
            col("start_min"),
            col("end_min"),
        );
        let mut by: BTreeMap<u32, Vec<Value>> = BTreeMap::new();
        for r in 0..t.row_count as usize {
            by.entry(tr[r] as u32).or_default().push(json!({
                "rt_min": rt[r], "area": a[r], "start_min": s0[r], "end_min": s1[r],
            }));
        }
        for (k, peaks) in by {
            let Some(chrom) = trace(
                ds.as_mut(),
                Target::Trace {
                    trace: k,
                    channel: None,
                },
            ) else {
                continue;
            };
            out.push(Case {
                id: format!(
                    "{id} {}",
                    info.traces[k as usize].name.clone().unwrap_or_default()
                ),
                family: fam,
                chrom,
                // Chromeleon's areas are signal × min
                area_to_signal_s: Some(60.0),
                peaks,
            });
        }
    }
}

/// One vendor-integrated chromatogram.
struct Case {
    id: String,
    family: &'static str,
    chrom: Chromatogram,
    /// Vendor area × this = signal × seconds (None: the vendor's area unit is unknown, compare
    /// area % only).
    area_to_signal_s: Option<f64>,
    peaks: Vec<Value>,
}

/// The vendor data set of a corpus id.
fn family(id: &str) -> &'static str {
    if id.starts_with("mtbls1892-") {
        "LabSolutions HPLC-PDA (ANDI)"
    } else if id.starts_with("cheminfo-") {
        "ChemStation HPLC-DAD (ANDI)"
    } else if id.starts_with("chromhandler-001") {
        "ChemStation GC (Report.TXT)"
    } else if id.starts_with("gc2asm-") {
        "ChemStation GC (Result.xml)"
    } else if id.starts_with("chromhandler-rau") {
        "MSD ChemStation GC-MS TIC (RESULTS.CSV)"
    } else {
        "OpenLab CDS GC-FID (.rx)"
    }
}

fn read_json_dir(dir: &Path) -> Vec<(String, Value)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
        .into_iter()
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.ends_with(".json") && !n.ends_with(".xic.json")
        })
        .map(|p| {
            let id = p.file_stem().unwrap().to_string_lossy().to_string();
            let v = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            (id, v)
        })
        .filter(|(id, _)| only(id))
        .collect()
}

fn trace(ds: &mut dyn Dataset, target: Target) -> Option<Chromatogram> {
    let info = ds.info().ok()?;
    let mut req = ChromRequest::default();
    req.targets = vec![target];
    extract(ds, &info, &req, &ReadContext::default())
        .ok()
        .map(|mut o| o.chromatograms.remove(0))
}

/// Whether every vendor-integrated chromatogram (`corpus/oracle/quant`, `corpus/oracle/openlab`)
/// is on disk.
fn all_cases_present() -> bool {
    let files = corpus_dir();
    ["corpus/oracle/quant", "corpus/oracle/openlab"]
        .iter()
        .all(|d| {
            read_json_dir(&root().join(d))
                .iter()
                .all(|(_, o)| files.join(o["input"].as_str().unwrap()).exists())
        })
}

/// Every vendor-integrated chromatogram present in the corpus directory.
fn cases() -> Vec<Case> {
    let reg = registry();
    let files = corpus_dir();
    let mut out = Vec::new();
    for (id, o) in read_json_dir(&root().join("corpus/oracle/quant")) {
        let input = files.join(o["input"].as_str().unwrap());
        if !input.exists() {
            continue;
        }
        let (_, mut ds) = reg
            .open(&input)
            .unwrap_or_else(|e| panic!("{}: {e}", input.display()));
        let k = o["vendor"]["area_to_signal_s"].as_f64();
        if let Some(peaks) = o.get("peaks").and_then(Value::as_array) {
            let target = if o["signal"].get("tic").is_some() {
                Target::Tic
            } else {
                Target::Trace {
                    trace: o["signal"]["trace"].as_u64().unwrap_or(0) as u32,
                    channel: None,
                }
            };
            let chrom = trace(ds.as_mut(), target).unwrap();
            out.push(Case {
                id: id.clone(),
                family: family(&id),
                chrom,
                area_to_signal_s: k,
                peaks: peaks.clone(),
            });
        }
        if let Some(signals) = o.get("signals").and_then(Value::as_array) {
            let info = ds.info().unwrap();
            for s in signals {
                let name = s["trace_name"].as_str().unwrap();
                let tr = info
                    .traces
                    .iter()
                    .find(|x| {
                        x.name
                            .as_deref()
                            .is_some_and(|n| n.split([',', ' ']).next() == Some(name))
                    })
                    .unwrap_or_else(|| panic!("{id}: no trace named {name}"));
                let chrom = trace(
                    ds.as_mut(),
                    Target::Trace {
                        trace: tr.index,
                        channel: None,
                    },
                )
                .unwrap();
                out.push(Case {
                    id: format!("{id} {name}"),
                    family: family(&id),
                    chrom,
                    area_to_signal_s: k,
                    peaks: s["peaks"].as_array().unwrap().clone(),
                });
            }
        }
    }
    for (_, o) in read_json_dir(&root().join("corpus/oracle/openlab")) {
        let id = o["id"].as_str().unwrap().to_string();
        let input = files.join(o["input"].as_str().unwrap());
        if !input.exists() {
            continue;
        }
        let (_, mut ds) = reg
            .open(&input)
            .unwrap_or_else(|e| panic!("{}: {e}", input.display()));
        let info = ds.info().unwrap();
        for s in o["signals"].as_array().unwrap() {
            let name = s["signal"].as_str().unwrap();
            let Some(t) = info.traces.iter().find(|t| t.channels[0].name == name) else {
                continue;
            };
            let Some(chrom) = trace(
                ds.as_mut(),
                Target::Trace {
                    trace: t.index,
                    channel: None,
                },
            ) else {
                continue;
            };
            // the allotropy result sets hold signals cut to a few values: only peaks whose
            // limits lie inside a real signal are compared
            let n = chrom.rt_min.len();
            if n <= 100 {
                continue;
            }
            let (t0, t1) = (chrom.rt_min[0], chrom.rt_min[n - 1]);
            let peaks: Vec<Value> = s["peaks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| {
                    f(p, "start_min").is_some_and(|a| a >= t0)
                        && f(p, "end_min").is_some_and(|b| b <= t1)
                })
                .cloned()
                .collect();
            if peaks.is_empty() {
                continue;
            }
            out.push(Case {
                id: format!("{id} {name}"),
                family: family(&id),
                chrom,
                area_to_signal_s: Some(1.0),
                peaks,
            });
        }
    }
    chromeleon_cases(&mut out);
    out
}

/// Our peaks whose apex lies inside the vendor peak's limits (or near its retention time when
/// the vendor gives no limits).
fn matched<'a>(ours: &'a [Peak], vp: &Value) -> Vec<&'a Peak> {
    let rt = f(vp, "rt_min").unwrap();
    if let (Some(a), Some(b)) = (f(vp, "start_min"), f(vp, "end_min")) {
        let inside: Vec<&Peak> = ours
            .iter()
            .filter(|p| p.rt_min >= a && p.rt_min <= b)
            .collect();
        if inside.is_empty() {
            return ours
                .iter()
                .filter(|p| (p.rt_min - rt).abs() <= 0.02)
                .collect();
        }
        return inside;
    }
    let w = f(vp, "width_min").unwrap_or(0.05).max(0.02);
    ours.iter()
        .filter(|p| (p.rt_min - rt).abs() <= w)
        .min_by(|a, b| (a.rt_min - rt).abs().total_cmp(&(b.rt_min - rt).abs()))
        .into_iter()
        .collect()
}

/// Agreement of one mode with one vendor data set.
#[derive(Default, Clone)]
struct Agreement {
    vendor_peaks: usize,
    found: usize,
    rt: Vec<f64>,
    area: Vec<f64>,
    area_pct: Vec<f64>,
    /// Our peaks of at least 1 % of the matched area that match no vendor peak.
    extra: usize,
    worst: Vec<(f64, String)>,
}

impl Agreement {
    fn merge(&mut self, o: &Agreement) {
        self.vendor_peaks += o.vendor_peaks;
        self.found += o.found;
        self.rt.extend(&o.rt);
        self.area.extend(&o.area);
        self.area_pct.extend(&o.area_pct);
        self.extra += o.extra;
    }
}

/// Median, 95th percentile and maximum of |v|.
fn stats(v: &[f64]) -> (f64, f64, f64) {
    if v.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(f64::total_cmp);
    let q = |p: f64| a[((a.len() - 1) as f64 * p).round() as usize];
    (q(0.5), q(0.95), a[a.len() - 1])
}

fn params(mode: BaselineMode) -> PeakParams {
    let mut p = PeakParams::default();
    p.area_time_unit = AreaTimeUnit::S;
    p.baseline = mode;
    p
}

fn compare(case: &Case, ours: &[Peak], a: &mut Agreement) {
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for vp in &case.peaks {
        a.vendor_peaks += 1;
        let rt = f(vp, "rt_min").unwrap();
        let m = matched(ours, vp);
        let Some(top) = m.iter().max_by(|x, y| x.height.total_cmp(&y.height)) else {
            a.worst
                .push((f64::INFINITY, format!("{} {rt:.3} min: not found", case.id)));
            continue;
        };
        a.found += 1;
        a.rt.push(top.rt_min - rt);
        let area: f64 = m.iter().map(|q| q.area).sum();
        let va = f(vp, "area").unwrap();
        pairs.push((area, va));
        if let Some(k) = case.area_to_signal_s {
            let rel = area / (va * k) - 1.0;
            a.area.push(rel);
            a.worst.push((
                rel.abs(),
                format!(
                    "{} {rt:.3} min: area {area:.5} vs vendor {:.5} ({:+.1} %), ours {}",
                    case.id,
                    va * k,
                    100.0 * rel,
                    m.iter()
                        .map(|q| format!("{:.3}-{:.3} {}", q.start_min, q.end_min, q.baseline_code))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    // our sizeable peaks the vendor did not report (within the range the vendor integrated)
    let matched_area: f64 = pairs.iter().map(|p| p.0).sum();
    let (lo, hi) = case
        .peaks
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            let r = f(v, "rt_min").unwrap();
            (
                a.min(f(v, "start_min").unwrap_or(r)),
                b.max(f(v, "end_min").unwrap_or(r)),
            )
        });
    a.extra += ours
        .iter()
        .filter(|q| q.rt_min >= lo && q.rt_min <= hi && q.area >= 0.01 * matched_area)
        .filter(|q| {
            !case
                .peaks
                .iter()
                .any(|vp| matched(std::slice::from_ref(*q), vp).len() == 1)
        })
        .count();
    // unit-free: area % among the vendor's peaks that both found
    let (so, sv) = pairs
        .iter()
        .fold((0.0, 0.0), |(x, y), (o, v)| (x + o, y + v));
    if so > 0.0 && sv > 0.0 {
        for (o, v) in pairs {
            a.area_pct.push(100.0 * (o / so - v / sv));
        }
    }
}

fn dump(dir: &Path, case: &Case, tables: &[(BaselineMode, Vec<Peak>)]) {
    let name: String = case
        .id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let v = json!({
        "id": case.id,
        "family": case.family,
        "area_to_signal_s": case.area_to_signal_s,
        "t": case.chrom.rt_min,
        "y": case.chrom.intensity,
        "vendor": case.peaks,
        "ours": tables.iter().map(|(m, p)| (m.id().to_string(), serde_json::to_value(p).unwrap())).collect::<serde_json::Map<_, _>>(),
    });
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{name}.json")), v.to_string()).unwrap();
}

const MODES: [BaselineMode; 4] = [
    BaselineMode::Drop,
    BaselineMode::Valley,
    BaselineMode::Tangent,
    BaselineMode::Auto,
];

#[test]
fn vendor_agreement() {
    let cases = cases();
    if cases.is_empty() {
        eprintln!("no vendor-integrated chromatograms present; skipped");
        return;
    }
    let dump_dir = std::env::var_os("PEAK_AGREEMENT_DUMP").map(PathBuf::from);
    let worst_n: usize = std::env::var("PEAK_AGREEMENT_WORST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // mode → family → agreement
    let mut by: BTreeMap<&str, BTreeMap<&str, Agreement>> = BTreeMap::new();
    for case in &cases {
        let mut tables = Vec::new();
        for mode in MODES {
            let t = find_peaks(&case.chrom.rt_min, &case.chrom.intensity, &params(mode)).unwrap();
            let a = by
                .entry(mode.id())
                .or_default()
                .entry(case.family)
                .or_default();
            compare(case, &t.peaks, a);
            tables.push((mode, t.peaks));
        }
        if let Some(d) = &dump_dir {
            dump(d, case, &tables);
        }
    }
    let n_peaks: usize = cases.iter().map(|c| c.peaks.len()).sum();
    println!(
        "vendor agreement: {} chromatograms, {n_peaks} vendor peaks (default baseline: {})",
        cases.len(),
        PeakParams::default().baseline.id()
    );
    let pct = |v: &[f64]| {
        let (m, p, x) = stats(v);
        format!("{:>6.2} {:>7.1} {:>7.0}", 100.0 * m, 100.0 * p, 100.0 * x)
    };
    let pts = |v: &[f64]| {
        let (m, p, x) = stats(v);
        format!("{m:>5.2} {p:>5.2} {x:>5.1}")
    };
    let rt = |v: &[f64]| {
        let (m, p, _) = stats(v);
        format!("{m:>6.4} {p:>6.4}")
    };
    println!(
        "  {:<40} {:<8} {:>9}  {:>13}  {:>22}  {:>17}  {:>5}",
        "vendor data set",
        "baseline",
        "found",
        "RT Δ med p95",
        "area % err med p95 max",
        "area-% Δ med p95 max",
        "extra"
    );
    let mut families: Vec<&str> = cases.iter().map(|c| c.family).collect();
    families.sort_unstable();
    families.dedup();
    let mut all: BTreeMap<&str, Agreement> = BTreeMap::new();
    for fam in &families {
        for mode in MODES {
            let a = &by[mode.id()][fam];
            all.entry(mode.id()).or_default().merge(a);
            println!(
                "  {fam:<40} {:<8} {:>4}/{:<4}  {}  {}  {}  {:>5}",
                mode.id(),
                a.found,
                a.vendor_peaks,
                rt(&a.rt),
                if a.area.is_empty() {
                    format!("{:>22}", "(no vendor unit)")
                } else {
                    pct(&a.area)
                },
                pts(&a.area_pct),
                a.extra
            );
        }
    }
    for mode in MODES {
        let a = &all[mode.id()];
        println!(
            "  {:<40} {:<8} {:>4}/{:<4}  {}  {}  {}  {:>5}",
            "all",
            mode.id(),
            a.found,
            a.vendor_peaks,
            rt(&a.rt),
            pct(&a.area),
            pts(&a.area_pct),
            a.extra
        );
    }
    if worst_n > 0 {
        for mode in MODES {
            let mut w: Vec<(f64, String)> = by[mode.id()]
                .values()
                .flat_map(|a| a.worst.iter().cloned())
                .collect();
            w.sort_by(|a, b| b.0.total_cmp(&a.0));
            println!("  worst ({}):", mode.id());
            for (_, s) in w.iter().take(worst_n) {
                println!("    {s}");
            }
        }
    }
    // The default must not regress any vendor data set against the drop-line baseline (within
    // the noise of small samples) and must be closer to the vendors overall. Chromeleon's stored
    // results are measured (book/src/guides/quantitation.md) but not part of this guard: they were added
    // after the default was designed, and on them no mode is closer than the others (the
    // vendor's method-specific detection finds many peaks near the noise that no default does).
    let default = PeakParams::default().baseline;
    assert_eq!(default, BaselineMode::Auto, "the documented default");
    let mut problems = Vec::new();
    for fam in families.iter().filter(|f| !f.starts_with("Chromeleon")) {
        let (d, a) = (&by["drop"][fam], &by[default.id()][fam]);
        if a.found < d.found {
            problems.push(format!("{fam}: found {} < drop {}", a.found, d.found));
        }
        let mut worse = |name: &str, x: &[f64], y: &[f64], rel: f64, abs: f64| {
            if x.is_empty() {
                return;
            }
            let (dm, dp, _) = stats(x);
            let (am, ap, _) = stats(y);
            if am > dm * (1.0 + rel) + abs {
                problems.push(format!("{fam}: {name} median {am} > drop {dm}"));
            }
            if ap > dp * (1.0 + rel) + abs {
                problems.push(format!("{fam}: {name} p95 {ap} > drop {dp}"));
            }
        };
        worse("area error", &d.area, &a.area, 0.1, 0.005);
        worse("area-% difference", &d.area_pct, &a.area_pct, 0.1, 0.25);
        let (dm, _, _) = stats(&d.rt);
        let (am, _, _) = stats(&a.rt);
        if am > dm + 0.001 {
            problems.push(format!("{fam}: RT median {am} > drop {dm}"));
        }
    }
    let mut design: BTreeMap<&str, Agreement> = BTreeMap::new();
    for fam in families.iter().filter(|f| !f.starts_with("Chromeleon")) {
        for mode in MODES {
            design
                .entry(mode.id())
                .or_default()
                .merge(&by[mode.id()][fam]);
        }
    }
    let (d, a) = (&design["drop"], &design[default.id()]);
    let ((_, dp, _), (_, ap, _)) = (stats(&d.area), stats(&a.area));
    // The overall margin over drop is a claim about the whole set; a subset (CI's smoke tier)
    // checks only the per-family bounds above.
    let whole_set = all_cases_present();
    if whole_set && ap > 0.5 * dp {
        problems.push(format!(
            "overall area p95 {ap} is not well below drop's {dp}"
        ));
    }
    let ((_, dp, _), (_, ap, _)) = (stats(&d.area_pct), stats(&a.area_pct));
    if whole_set && ap > 0.5 * dp {
        problems.push(format!(
            "overall area-% p95 {ap} is not well below drop's {dp}"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
