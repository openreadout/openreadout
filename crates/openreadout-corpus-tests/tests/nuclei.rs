//! Object counting (`stats` objects) against hand-annotated nuclei: BBBC039 (U2OS nuclei,
//! Hoechst) and the BBBC038 training set (the 2018 Data Science Bowl: fluorescence,
//! brightfield and H&E tissue).
//!
//! Each predicted object is matched to the annotated nucleus it overlaps with an
//! intersection-over-union above 0.5 (such a match is unique). The test prints precision, recall
//! and F1 at IoU 0.5 and the count error per group, and fails when F1 falls below a floor per
//! group, so a change that makes counting worse shows up.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test nuclei -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `NUCLEI_LIMIT=N` evaluates the first N
//! images of each set; `NUCLEI_DUMP=DIR` writes a TSV of per-image results there, and with
//! `NUCLEI_SHOW=PREFIX,...` pictures of those images with both outlines.
#![cfg(feature = "corpus")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use openreadout_core::PlaneIndex;
use rayon::prelude::*;

#[path = "support/registry.rs"]
mod registry;

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

/// One image to segment and its annotation.
struct Case {
    name: String,
    group: &'static str,
    w: usize,
    h: usize,
    /// One plane per component (1 grey, 3 RGB).
    comps: Vec<Vec<f64>>,
    pixel_um: Option<f64>,
    truth: Vec<u32>,
}

/// Label the 8-connected regions of each distinct non-zero value (one nucleus each).
fn label_by_value(v: &[u32], w: usize, h: usize) -> Vec<u32> {
    let mut out = vec![0u32; w * h];
    let mut next = 0u32;
    let mut stack = Vec::new();
    for s in 0..w * h {
        if v[s] == 0 || out[s] != 0 {
            continue;
        }
        next += 1;
        out[s] = next;
        stack.push(s);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if out[j] == 0 && v[j] == v[s] {
                        out[j] = next;
                        stack.push(j);
                    }
                }
            }
        }
    }
    out
}

fn read_png(path: &Path) -> (usize, usize, usize, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    ));
    let mut r = dec.read_info().expect("png header");
    let mut buf = vec![0; r.output_buffer_size().expect("png size")];
    let info = r.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    let spp = info.color_type.samples();
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "{}", path.display());
    (info.width as usize, info.height as usize, spp, buf)
}

fn bbbc039() -> Vec<Case> {
    let dir = corpus_dir();
    let list = dir.join("bbbc039-metadata/metadata/test.txt");
    let Ok(names) = std::fs::read_to_string(&list) else {
        eprintln!("skip BBBC039: {} missing", list.display());
        return Vec::new();
    };
    let reg = registry::registry();
    names
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|png_name| {
            let png_name = png_name.trim();
            let stem = png_name.trim_end_matches(".png");
            let (w, h, spp, m) = read_png(&dir.join("bbbc039-masks/masks").join(png_name));
            let red: Vec<u32> = m.chunks_exact(spp).map(|p| u32::from(p[0])).collect();
            let truth = label_by_value(&red, w, h);
            let tif = dir
                .join("bbbc039-images/images")
                .join(format!("{stem}.tif"));
            let (_, mut ds) = reg.open(&tif).expect("open BBBC039 TIFF");
            let info = ds.info().expect("info");
            let im = &info.images[0];
            let p = ds
                .read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 })
                .expect("plane");
            let bps = p.pixel_type.bytes_per_sample();
            let v: Vec<f64> = p
                .data
                .chunks_exact(bps)
                .map(|c| p.pixel_type.sample_f64(c))
                .collect();
            assert_eq!((im.size_x as usize, im.size_y as usize), (w, h));
            Case {
                name: stem.to_string(),
                group: "bbbc039 fluorescence",
                w,
                h,
                comps: vec![v],
                pixel_um: im.physical_size.x,
                truth,
            }
        })
        .collect()
}

fn bbbc038() -> Vec<Case> {
    let dir = corpus_dir().join("bbbc038-stage1-train");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skip BBBC038: {} missing", dir.display());
        return Vec::new();
    };
    let mut ids: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("images").is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids.par_iter()
        .map(|id| {
            let (w, h, spp, px) = read_png(&dir.join(id).join("images").join(format!("{id}.png")));
            let mut truth = vec![0u32; w * h];
            let mut masks: Vec<PathBuf> = std::fs::read_dir(dir.join(id).join("masks"))
                .expect("masks")
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            masks.sort();
            for (k, m) in masks.iter().enumerate() {
                let (mw, mh, mspp, mv) = read_png(m);
                assert_eq!((mw, mh), (w, h));
                for (i, p) in mv.chunks_exact(mspp).enumerate() {
                    if p[0] > 0 {
                        truth[i] = k as u32 + 1;
                    }
                }
            }
            let rgb: Vec<[f64; 3]> = px
                .chunks_exact(spp)
                .map(|p| {
                    if spp >= 3 {
                        [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
                    } else {
                        [f64::from(p[0]); 3]
                    }
                })
                .collect();
            let grey = rgb.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
            let mean = rgb.iter().map(|p| p[0]).sum::<f64>() / rgb.len() as f64;
            let (group, comps) = if grey {
                let v: Vec<f64> = rgb.iter().map(|p| p[0]).collect();
                (
                    if mean > 100.0 {
                        "bbbc038 brightfield"
                    } else {
                        "bbbc038 fluorescence"
                    },
                    vec![v],
                )
            } else {
                (
                    "bbbc038 stained (H&E)",
                    (0..3).map(|k| rgb.iter().map(|p| p[k]).collect()).collect(),
                )
            };
            Case {
                name: id.clone(),
                group,
                w,
                h,
                comps,
                pixel_um: None,
                truth,
            }
        })
        .collect()
}

/// The predicted label image of one case: the default count.
fn predict(c: &Case) -> Vec<u32> {
    predict_d(c).0
}

/// The predicted label image, the expected diameter used, the objects' fit and whether the
/// count has low confidence.
fn predict_d(c: &Case) -> (Vec<u32>, f64, f64, bool) {
    let comps: Vec<Vec<f32>> = c
        .comps
        .iter()
        .map(|p| p.iter().map(|v| *v as f32).collect())
        .collect();
    let req = openreadout_core::measure::ObjectsRequest::default();
    openreadout_core::measure::label_plane(&comps, c.w, c.h, c.pixel_um, &req).map_or_else(
        |_| (vec![0; c.w * c.h], 0.0, 0.0, true),
        |(lab, d, fit, conf)| {
            (
                lab,
                d,
                fit,
                conf == openreadout_core::measure::CountConfidence::Low,
            )
        },
    )
}

/// Median diameter (of a disc of the same area) of the annotated nuclei, in pixels.
fn truth_diameter(t: &[u32]) -> f64 {
    let mut area: HashMap<u32, usize> = HashMap::new();
    for &l in t.iter().filter(|l| **l > 0) {
        *area.entry(l).or_default() += 1;
    }
    let mut d: Vec<f64> = area
        .values()
        .map(|a| 2.0 * (*a as f64 / std::f64::consts::PI).sqrt())
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    d.sort_by(f64::total_cmp);
    d[d.len() / 2]
}

#[derive(Default, Clone, Copy)]
struct Score {
    tp: usize,
    fp: usize,
    fn_: usize,
    truth: usize,
    found: usize,
    /// Missed nuclei: no found object covers a fifth of them.
    missed: usize,
    /// Missed nuclei covered mostly by an object that also covers another nucleus.
    merged: usize,
}

fn score(pred: &[u32], truth: &[u32]) -> Score {
    let mut area_p: HashMap<u32, usize> = HashMap::new();
    let mut area_t: HashMap<u32, usize> = HashMap::new();
    let mut inter: HashMap<(u32, u32), usize> = HashMap::new();
    for (&p, &t) in pred.iter().zip(truth) {
        if p > 0 {
            *area_p.entry(p).or_default() += 1;
        }
        if t > 0 {
            *area_t.entry(t).or_default() += 1;
        }
        if p > 0 && t > 0 {
            *inter.entry((p, t)).or_default() += 1;
        }
    }
    let tp = inter
        .iter()
        .filter(|((p, t), n)| {
            let u = area_p[p] + area_t[t] - **n;
            **n as f64 / u as f64 > 0.5
        })
        .count();
    let matched: std::collections::HashSet<u32> = inter
        .iter()
        .filter(|((p, t), n)| **n as f64 / (area_p[p] + area_t[t] - **n) as f64 > 0.5)
        .map(|((_, t), _)| *t)
        .collect();
    // for each nucleus, the object covering most of it
    let mut best: HashMap<u32, (u32, usize)> = HashMap::new();
    let mut covers: HashMap<u32, usize> = HashMap::new();
    for ((p, t), n) in &inter {
        let e = best.entry(*t).or_insert((*p, 0));
        if *n > e.1 {
            *e = (*p, *n);
        }
        if *n as f64 >= 0.2 * area_t[t] as f64 {
            *covers.entry(*p).or_default() += 1;
        }
    }
    let (mut missed, mut merged) = (0, 0);
    for (t, a) in &area_t {
        if matched.contains(t) {
            continue;
        }
        match best.get(t) {
            Some((p, n)) if *n as f64 >= 0.2 * *a as f64 => {
                if covers.get(p).copied().unwrap_or(0) >= 2 {
                    merged += 1;
                }
            }
            _ => missed += 1,
        }
    }
    Score {
        tp,
        fp: area_p.len() - tp,
        fn_: area_t.len() - tp,
        truth: area_t.len(),
        found: area_p.len(),
        missed,
        merged,
    }
}

#[test]
fn nuclei_against_annotations() {
    let limit: usize = std::env::var("NUCLEI_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let mut cases = bbbc039();
    cases.truncate(limit);
    let mut b38 = bbbc038();
    b38.truncate(limit);
    cases.extend(b38);
    if cases.is_empty() {
        return;
    }
    let t0 = std::time::Instant::now();
    let results: Vec<(&Case, Score, f64, f64, bool)> = cases
        .par_iter()
        .map(|c| {
            let (p, d, fit, low) = predict_d(c);
            (c, score(&p, &c.truth), d, fit, low)
        })
        .collect();
    let secs = t0.elapsed().as_secs_f64();
    if let (Some(d), Ok(show)) = (
        std::env::var_os("NUCLEI_DUMP"),
        std::env::var("NUCLEI_SHOW"),
    ) {
        for c in cases
            .iter()
            .filter(|c| show.split(',').any(|s| c.name.starts_with(s)))
        {
            overlay(
                c,
                &predict(c),
                &Path::new(&d).join(format!("{}.png", &c.name[..12.min(c.name.len())])),
            );
        }
    }
    if let Some(d) = std::env::var_os("NUCLEI_DUMP") {
        let mut tsv =
            String::from("group\timage\ttruth\tfound\ttp\tfp\tfn\td_used\td_truth\tfit\tlow\n");
        for (c, s, d, fit, low) in &results {
            tsv.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{:.3}\t{}\n",
                c.group,
                c.name,
                s.truth,
                s.found,
                s.tp,
                s.fp,
                s.fn_,
                d,
                truth_diameter(&c.truth),
                fit,
                low
            ));
        }
        std::fs::write(Path::new(&d).join("nuclei.tsv"), tsv).expect("write dump");
    }
    // images the count flags as low confidence, and the count error of the others
    let mut flagged: HashMap<&str, (usize, Vec<f64>)> = HashMap::new();
    for (c, s, _, _, low) in &results {
        let e = flagged.entry(c.group).or_default();
        if *low {
            e.0 += 1;
        } else if s.truth > 0 {
            e.1.push((s.found as f64 - s.truth as f64).abs() / s.truth as f64);
        }
    }
    let results: Vec<(&Case, Score)> = results.into_iter().map(|(c, s, ..)| (c, s)).collect();
    let mut groups: Vec<&str> = results.iter().map(|r| r.0.group).collect();
    groups.sort_unstable();
    groups.dedup();
    println!(
        "{:<24} {:>6} {:>8} {:>8} {:>6} {:>6} {:>6} {:>10} {:>10}",
        "group", "images", "truth", "found", "P", "R", "F1", "|count err|", "count bias"
    );
    let mut failures = Vec::new();
    for g in groups {
        let rs: Vec<&(&Case, Score)> = results.iter().filter(|r| r.0.group == g).collect();
        let s = rs.iter().fold(Score::default(), |a, r| Score {
            tp: a.tp + r.1.tp,
            fp: a.fp + r.1.fp,
            fn_: a.fn_ + r.1.fn_,
            truth: a.truth + r.1.truth,
            found: a.found + r.1.found,
            missed: a.missed + r.1.missed,
            merged: a.merged + r.1.merged,
        });
        let p = s.tp as f64 / s.found.max(1) as f64;
        let r = s.tp as f64 / s.truth.max(1) as f64;
        let f1 = if p + r > 0.0 {
            2.0 * p * r / (p + r)
        } else {
            0.0
        };
        let errs: Vec<f64> = rs
            .iter()
            .filter(|r| r.1.truth > 0)
            .map(|r| (r.1.found as f64 - r.1.truth as f64) / r.1.truth as f64)
            .collect();
        let abs = errs.iter().map(|e| e.abs()).sum::<f64>() / errs.len().max(1) as f64;
        let bias = errs.iter().sum::<f64>() / errs.len().max(1) as f64;
        println!(
            "{g:<24} {:>6} {:>8} {:>8} {p:>6.3} {r:>6.3} {f1:>6.3} {:>9.1}% {:>9.1}%",
            rs.len(),
            s.truth,
            s.found,
            abs * 100.0,
            bias * 100.0
        );
        println!(
            "{:<24} not found {} ({:.1}%), merged with a neighbour {} ({:.1}%), outline off {} ({:.1}%)",
            "",
            s.missed,
            100.0 * s.missed as f64 / s.truth.max(1) as f64,
            s.merged,
            100.0 * s.merged as f64 / s.truth.max(1) as f64,
            s.fn_ - s.missed - s.merged,
            100.0 * (s.fn_ - s.missed - s.merged) as f64 / s.truth.max(1) as f64
        );
        if let Some((n, errs)) = flagged.get(g) {
            println!(
                "{:<24} low confidence {n} images; |count err| of the others {:.1}%",
                "",
                100.0 * errs.iter().sum::<f64>() / errs.len().max(1) as f64
            );
        }
        let floor = floor(g);
        if f1 < floor {
            failures.push(format!("{g}: F1 {f1:.3} below {floor}"));
        }
    }
    println!("{} images in {secs:.1} s", results.len());
    assert!(failures.is_empty(), "{failures:?}");
}

/// The image (grey, contrast stretched) with annotated outlines in green and predicted ones
/// in red, for looking at failures (`NUCLEI_SHOW=<image name prefix>`).
fn overlay(c: &Case, pred: &[u32], out: &Path) {
    let (w, h) = (c.w, c.h);
    let grey: Vec<f64> = (0..w * h)
        .map(|i| c.comps.iter().map(|p| p[i]).sum::<f64>() / c.comps.len() as f64)
        .collect();
    let mut sorted = grey.clone();
    sorted.sort_by(f64::total_cmp);
    let lo = sorted[sorted.len() / 1000];
    let hi = sorted[sorted.len() - 1 - sorted.len() / 1000].max(lo + 1.0);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        let v = ((grey[i] - lo) / (hi - lo) * 255.0).clamp(0.0, 255.0) as u8;
        let rim = |lab: &[u32]| {
            let (x, y) = (i % w, i / w);
            lab[i] != 0
                && (x == 0
                    || y == 0
                    || x + 1 == w
                    || y + 1 == h
                    || lab[i - 1] != lab[i]
                    || lab[i + 1] != lab[i]
                    || lab[i - w] != lab[i]
                    || lab[i + w] != lab[i])
        };
        if rim(pred) {
            rgb.extend([255, 40, 40]);
        } else if rim(&c.truth) {
            rgb.extend([40, 255, 40]);
        } else {
            rgb.extend([v, v, v]);
        }
    }
    // small images enlarged so the outlines can be told apart
    let k = (800 / w.max(h)).max(1);
    let rgb: Vec<u8> = (0..h * k)
        .flat_map(|y| (0..w * k).map(move |x| (y / k) * w + x / k))
        .flat_map(|i| [rgb[3 * i], rgb[3 * i + 1], rgb[3 * i + 2]])
        .collect();
    let (w, h) = (w * k, h * k);
    let f = std::fs::File::create(out).expect("create overlay");
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()
        .expect("png header")
        .write_image_data(&rgb)
        .expect("png data");
}

/// Lowest F1 at IoU 0.5 accepted per group, a little under what the counting reaches now.
fn floor(group: &str) -> f64 {
    match group {
        "bbbc039 fluorescence" => 0.88,
        "bbbc038 fluorescence" => 0.82,
        "bbbc038 stained (H&E)" => 0.55,
        _ => 0.2,
    }
}
