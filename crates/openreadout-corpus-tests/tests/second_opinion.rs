//! Differential cross-check (docs/assurance.md): where a second independent reader exists, the
//! corpus compares OpenReadout against it as well as against the primary oracle. The second
//! opinions are `corpus/oracle/second/<id>.json` (oracle/second_opinion.py: pylibCZIrw for CZI,
//! readlif for LIF, Bio-Formats for ND2, all run as black boxes). A plane where OpenReadout and the second reader
//! disagree must be adjudicated in `corpus/oracle/second/adjudications.toml` (which reader is
//! right and why), otherwise the test fails. Planes the primary oracle also disagrees on are
//! the main corpus test's failures and are not repeated here.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test second_opinion`.
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_core::reader::PlaneIndex;
use openreadout_corpus_tests::oracle_json;
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    filename: String,
}

#[derive(Deserialize)]
struct Second {
    id: String,
    #[serde(default)]
    reader: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    images: Vec<SecondImage>,
}
#[derive(Deserialize)]
struct SecondImage {
    index: u32,
    /// The image's name in the second reader (LIF: the element path without the file name),
    /// used to find our image when the two readers number images differently.
    #[serde(default)]
    name: Option<String>,
    size_x: u32,
    size_y: u32,
    #[serde(default)]
    skip: Option<String>,
    #[serde(default)]
    planes: Vec<SecondPlane>,
}
#[derive(Deserialize)]
struct SecondPlane {
    c: u32,
    z: u32,
    t: u32,
    xxh3: String,
}

#[derive(Deserialize, Default)]
struct Adjudications {
    #[serde(default)]
    file: BTreeMap<String, Adjudication>,
    /// Second opinions recorded inside primary oracle JSONs, by oracle file stem.
    #[serde(default)]
    recorded: BTreeMap<String, Adjudication>,
}
#[derive(Deserialize)]
struct Adjudication {
    /// Which reader is right: `openreadout`, `second`, `neither`.
    right: String,
    why: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_oir::OirReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_em::MrcReader))
        .with(Box::new(openreadout_em::DmReader))
}

fn hash(b: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(b))
}

#[test]
fn second_opinions_agree() {
    let root = root();
    let dir = root.join("corpus/oracle/second");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return;
    };
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files: BTreeMap<&str, &str> = manifest
        .file
        .iter()
        .map(|e| (e.id.as_str(), e.filename.as_str()))
        .collect();
    let adjudications: Adjudications = std::fs::read_to_string(dir.join("adjudications.toml"))
        .map(|t| toml::from_str(&t).unwrap())
        .unwrap_or_default();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let (mut agree, mut adjudicated, mut problems) = (0usize, 0usize, Vec::new());
    let mut regrouped: Vec<String> = Vec::new();
    let mut refused = 0usize;
    let mut paths: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    for p in paths {
        let s: Second = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        if only.as_ref().is_some_and(|o| !s.id.contains(o.as_str())) || s.error.is_some() {
            continue;
        }
        let Some(file) = files.get(s.id.as_str()) else {
            problems.push(format!("{}: not in the manifest", s.id));
            continue;
        };
        let path = files_dir.join(file);
        if !path.exists() {
            continue;
        }
        let (_, mut ds) = reg.open(&path).unwrap();
        let info = ds.info().unwrap();
        // Bio-Formats numbers series its own way (pyramid levels, labels and macro images as
        // series, companion files grouped differently): compare only when the image counts
        // agree, and say which files were left out.
        if s.reader.starts_with("Bio-Formats") && s.images.len() != info.images.len() {
            regrouped.push(format!(
                "{} ({} series, {} images)",
                s.id,
                s.images.len(),
                info.images.len()
            ));
            continue;
        }
        let mut differ = Vec::new();
        for im in s.images.iter().filter(|i| i.skip.is_none()) {
            // By name when the second reader names images (readlif leaves out images it
            // cannot read, so its indices shift), else by index.
            let found = match &im.name {
                Some(n) => info.images.iter().find(|o| {
                    o.name
                        .as_deref()
                        .is_some_and(|on| on == n || on.ends_with(&format!("/{n}")))
                }),
                None => info.images.get(im.index as usize),
            };
            let Some(ours) = found else {
                differ.push(format!("image {} missing in ours", im.index));
                continue;
            };
            let index = ours.index;
            if (ours.size_x, ours.size_y) != (im.size_x, im.size_y) {
                differ.push(format!(
                    "image {}: {}x{} (ours) vs {}x{} ({})",
                    im.index, ours.size_x, ours.size_y, im.size_x, im.size_y, s.reader
                ));
                continue;
            }
            for pl in &im.planes {
                let got = ds.read_plane(
                    index,
                    PlaneIndex {
                        c: pl.c,
                        z: pl.z,
                        t: pl.t,
                    },
                );
                match got {
                    Ok(plane) if hash(&plane.data) == pl.xxh3 => agree += 1,
                    // Right or refuse: a plane we decline to read whole (exit 6, e.g. above
                    // 4 GiB, read by region instead) is not a disagreement.
                    Err(e) if e.exit_code() == 6 => refused += 1,
                    Ok(_) => differ.push(format!(
                        "image {index} c{} z{} t{}: plane differs",
                        pl.c, pl.z, pl.t
                    )),
                    Err(e) => differ.push(format!(
                        "image {index} c{} z{} t{}: ours failed ({e})",
                        pl.c, pl.z, pl.t
                    )),
                }
            }
        }
        if differ.is_empty() {
            continue;
        }
        match adjudications.file.get(&s.id) {
            Some(a) => {
                adjudicated += differ.len();
                println!(
                    "adjudicated {} ({} differences with {}): right = {}; {}",
                    s.id,
                    differ.len(),
                    s.reader,
                    a.right,
                    a.why
                );
            }
            None => problems.push(format!(
                "{} disagrees with {} on {} plane(s), not adjudicated: {}",
                s.id,
                s.reader,
                differ.len(),
                differ
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
        }
    }
    println!(
        "second opinions: {agree} planes agree, {adjudicated} adjudicated differences, {refused} refused by us (exit 6)"
    );
    println!(
        "not compared (Bio-Formats groups series differently): {} files: {}",
        regrouped.len(),
        regrouped.join(", ")
    );
    assert!(
        problems.is_empty(),
        "{} files:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Collect second-opinion verdicts recorded inside an oracle JSON: booleans whose key contains
/// `agree` (`agrees_with_flowio`, `absorbance_agrees`, `oiffile_agrees`, ...) and
/// `planes_disagree` counts.
fn recorded_flags(v: &serde_json::Value, path: &str, out: &mut Vec<(String, bool)>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                let p = format!("{path}/{k}");
                match x {
                    serde_json::Value::Bool(b) if k.contains("agree") => out.push((p, *b)),
                    serde_json::Value::Number(n) if k == "planes_disagree" => {
                        out.push((p, n.as_u64() == Some(0)));
                    }
                    _ => recorded_flags(x, &p, out),
                }
            }
        }
        serde_json::Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                recorded_flags(x, &format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

/// Oracles that ran a second reader record whether it agreed with the primary (FCS: fcsparser
/// against FlowIO; OPUS: brukeropusreader against brukeropus; OIB/OIF: oiffile against
/// Bio-Formats; plates: Bio-Formats against tifffile, ...). Every recorded disagreement must be
/// adjudicated, so a primary oracle that is itself wrong cannot hide a reader bug.
#[test]
fn recorded_second_opinions() {
    let root = root();
    let oracle = root.join("corpus/oracle");
    let adjudications: Adjudications =
        std::fs::read_to_string(oracle.join("second/adjudications.toml"))
            .map(|t| toml::from_str(&t).unwrap())
            .unwrap_or_default();
    // `<id>.json`, or `<id>.json.gz` over 1 MiB (logical `.json` paths either way)
    let mut paths = oracle_json::files(&oracle, ".json");
    for dir in std::fs::read_dir(&oracle).unwrap().filter_map(Result::ok) {
        let p = dir.path();
        if p.is_dir() {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name == "heldout" || name == "second" {
                continue;
            }
            paths.extend(oracle_json::files(&p, ".json"));
        }
    }
    paths.sort();
    let (mut agree, mut problems) = (0usize, Vec::new());
    for p in paths {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(
            &oracle_json::read_to_string(&p).unwrap_or_default(),
        ) else {
            continue;
        };
        let mut flags = Vec::new();
        recorded_flags(&v, "", &mut flags);
        let stem = p
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let bad: Vec<&String> = flags.iter().filter(|(_, ok)| !ok).map(|(k, _)| k).collect();
        agree += flags.len() - bad.len();
        if !bad.is_empty() && !adjudications.recorded.contains_key(&stem) {
            problems.push(format!(
                "{stem}: second opinion disagrees at {bad:?}, not adjudicated"
            ));
        }
    }
    for (stem, a) in &adjudications.recorded {
        assert!(
            ["openreadout", "second", "neither"].contains(&a.right.as_str()) && !a.why.is_empty(),
            "{stem}: adjudication needs right = openreadout|second|neither and a why"
        );
    }
    println!(
        "recorded second opinions: {agree} agree, {} adjudicated",
        adjudications.recorded.len()
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
