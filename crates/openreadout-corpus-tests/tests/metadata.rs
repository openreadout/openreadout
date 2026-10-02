//! Metadata conformance: every corpus file that opens must follow the normalization rules of
//! `book/src/guides/metadata.md` (units, timestamps, colours, wavelengths, index semantics,
//! dimension order, per-frame record vocabulary, provenance keys), and its derived `experiment`
//! must be well-formed (`book/src/guides/metadata.md`: every value has provenance, every term is
//! in the curated table, every unit has its UCUM code, `sample.source_field` is set whenever
//! `sample.id` is). The experiment walk also prints fill rates per family, and per format as
//! a Markdown table.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test metadata -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]

#[path = "support/registry.rs"]
mod shared_registry;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use openreadout_core::experiment::{self, Experiment};
use openreadout_core::model::{ChannelInfo, FileInfo, ImageInfo};
use openreadout_core::vocab;
use openreadout_core::{Error, Registry};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
}

/// Provenance keys that no corpus file exercises yet, and why that is expected.
const UNREALIZED_OK: &[(&str, &str, &str)] = &[
    (
        "chromeleon",
        "traces[].extra.calibration_level",
        "no corpus Chromeleon injection is a calibration standard with a level set",
    ),
    (
        "nd2",
        "images[].extra.rois",
        "corpus ND2 files hold only empty ROI trees (docs/formats/nd2.md § ROIs)",
    ),
    (
        "fcs",
        "tables[].extra.analysis_segment",
        "no corpus FCS file has a non-empty ANALYSIS segment",
    ),
    (
        "chemstation",
        "traces[].extra.method_file",
        "the only corpus .D with an acqmeth.txt (mtbls75, GC-MSD) holds spectra, no traces (docs/formats/chemstation.md § Acquisition method text)",
    ),
];

/// Plausible physical pixel size in µm (1 pm .. 1 cm): atomic-resolution STEM images have
/// pixels of a few picometres (Velox and DM files in the corpus: 7.5 pm, 24 pm, 30 pm).
const SIZE_UM: (f64, f64) = (1e-6, 1e4);
/// Plausible light wavelength in nm (UV to near infrared).
const WAVELENGTH_NM: (f64, f64) = (100.0, 2000.0);

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
        .with(Box::new(openreadout_fcs::FcsReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_mzml::MzmlbReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
        .with(Box::new(openreadout_em::MrcReader))
        .with(Box::new(openreadout_em::DmReader))
        .with(Box::new(openreadout_em::SerReader))
        .with(Box::new(openreadout_em::EmdReader))
        .with(Box::new(openreadout_hdf5::ImsReader))
        .with(Box::new(openreadout_hdf5::NwbReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_zvi::ZviReader))
        .with(Box::new(openreadout_oif::OibReader))
        .with(Box::new(openreadout_oif::OifReader))
        .with(Box::new(openreadout_dcimg::DcimgReader))
        .with(Box::new(openreadout_qpcr::RdmlReader))
        .with(Box::new(openreadout_qpcr::EdsReader))
        .with(Box::new(openreadout_qpcr::PcrdReader))
        .with(Box::new(openreadout_qpcr::RexReader))
        .with(Box::new(openreadout_qpcr::IxoReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
}

// ---------- timestamps ----------

/// Parse `YYYY-MM-DDTHH:MM:SS[.f+][Z|±hh:mm]`; `Ok(true)` when it carries a zone designator.
#[allow(clippy::many_single_char_names)]
fn iso8601(s: &str) -> Result<bool, String> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        s.get(r)
            .filter(|t| t.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|t| t.parse().ok())
    };
    let bad = || format!("{s:?} is not ISO-8601 (YYYY-MM-DDTHH:MM:SS[.fff][Z])");
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err(bad());
    }
    let (y, mo, d, h, mi, se) = (
        num(0..4).ok_or_else(bad)?,
        num(5..7).ok_or_else(bad)?,
        num(8..10).ok_or_else(bad)?,
        num(11..13).ok_or_else(bad)?,
        num(14..16).ok_or_else(bad)?,
        num(17..19).ok_or_else(bad)?,
    );
    if !(1900..=2100).contains(&y)
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || h > 23
        || mi > 59
        || se > 60
    {
        return Err(format!(
            "{s:?} has an out-of-range field (year must be 1900..2100)"
        ));
    }
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        if n == 0 {
            return Err(bad());
        }
        rest = &r[n..];
    }
    match rest {
        "" => Ok(false),
        "Z" => Ok(true),
        z if z.len() == 6
            && (z.starts_with('+') || z.starts_with('-'))
            && z.as_bytes()[3] == b':'
            && z[1..3]
                .bytes()
                .chain(z[4..].bytes())
                .all(|c| c.is_ascii_digit()) =>
        {
            Ok(true)
        }
        _ => Err(bad()),
    }
}

fn check_time(what: &str, s: &str, notes_mention_zone: bool, v: &mut Vec<String>) {
    match iso8601(s) {
        Ok(true) => {}
        Ok(false) if notes_mention_zone => {}
        Ok(false) => v.push(format!(
            "{what} {s:?} has no zone designator and no note says the source has none"
        )),
        Err(e) => v.push(format!("{what}: {e}")),
    }
}

// ---------- images ----------

fn is_colour(s: &str) -> bool {
    s.len() == 7
        && s.starts_with('#')
        && s[1..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c))
}

fn check_wavelength(what: &str, nm: f64, v: &mut Vec<String>) {
    if !(WAVELENGTH_NM.0..=WAVELENGTH_NM.1).contains(&nm) {
        v.push(format!("{what} {nm} nm outside {WAVELENGTH_NM:?}"));
    }
}

fn check_channel(i: u32, c: &ChannelInfo, v: &mut Vec<String>) {
    let w = |k: &str| format!("image {i} channel {} {k}", c.index);
    if let Some(col) = &c.color
        && !is_colour(col)
    {
        v.push(format!(
            "{} {col:?} is not #RRGGBB (upper-case hex)",
            w("color")
        ));
    }
    if let Some(x) = c.excitation_nm {
        check_wavelength(&w("excitation_nm"), x, v);
    }
    if let Some(x) = c.emission_nm {
        check_wavelength(&w("emission_nm"), x, v);
    }
    if let Some([a, b]) = c.emission_range_nm {
        check_wavelength(&w("emission_range_nm[0]"), a, v);
        check_wavelength(&w("emission_range_nm[1]"), b, v);
        if a > b {
            v.push(format!("{} [{a}, {b}] is reversed", w("emission_range_nm")));
        }
    }
    if let Some(e) = c.exposure_ms
        && !(e.is_finite() && e >= 0.0)
    {
        v.push(format!(
            "{} {e} is not a non-negative number",
            w("exposure_ms")
        ));
    }
}

fn check_image(im: &ImageInfo, zone_note: bool, v: &mut Vec<String>) {
    let i = im.index;
    if im.physical_size.unit != "µm" {
        v.push(format!(
            "image {i} physical_size.unit {:?} != µm",
            im.physical_size.unit
        ));
    }
    for (ax, x) in [
        ("x", im.physical_size.x),
        ("y", im.physical_size.y),
        ("z", im.physical_size.z),
    ] {
        if let Some(x) = x
            && !(SIZE_UM.0..=SIZE_UM.1).contains(&x)
        {
            v.push(format!(
                "image {i} physical_size.{ax} = {x} µm outside {SIZE_UM:?}"
            ));
        }
    }
    if let Some(t) = im.time_increment_s
        && !(t.is_finite() && t > 0.0 && t < 1e7)
    {
        v.push(format!(
            "image {i} time_increment_s = {t} is not a positive plausible interval"
        ));
    }
    if let Some(a) = &im.acquired_at {
        check_time(&format!("image {i} acquired_at"), a, zone_note, v);
    }
    let expect = u64::from(im.size_z) * u64::from(im.size_c) * u64::from(im.size_t);
    if im.plane_count != expect {
        v.push(format!(
            "image {i} plane_count {} != size_z*size_c*size_t = {expect}",
            im.plane_count
        ));
    }
    if [im.size_x, im.size_y, im.size_z, im.size_c, im.size_t].contains(&0) {
        v.push(format!("image {i} has a zero-length axis"));
    }
    let d = &im.dimension_order;
    let mut sorted: Vec<char> = d.chars().collect();
    sorted.sort_unstable();
    if !d.starts_with("XY") || sorted != ['C', 'T', 'X', 'Y', 'Z'] {
        v.push(format!(
            "image {i} dimension_order {d:?} is not XY + a permutation of CZT"
        ));
    }
    let idx: Vec<u32> = im.channels.iter().map(|c| c.index).collect();
    if idx.iter().copied().ne(0..idx.len() as u32) {
        v.push(format!(
            "image {i} channel indices {idx:?} are not 0..n in order"
        ));
    }
    if !im.channels.is_empty() && im.channels.len() != im.size_c as usize {
        v.push(format!(
            "image {i} lists {} channels for size_c {}",
            im.channels.len(),
            im.size_c
        ));
    }
    for c in &im.channels {
        check_channel(i, c, v);
    }
    if !(1..=64).contains(&im.samples_per_pixel) {
        v.push(format!(
            "image {i} samples_per_pixel {}",
            im.samples_per_pixel
        ));
    }
    if im.pyramid_levels == 0 {
        v.push(format!("image {i} pyramid_levels is 0 (1 = no pyramid)"));
    }
    if let Some(m) = &im.mosaic
        && m.tile_count == 0
    {
        v.push(format!("image {i} mosaic with 0 tiles"));
    }
    if let Some(rois) = im.extra.get("rois").and_then(Value::as_array) {
        for r in rois {
            if let Some(c) = r.get("color").and_then(Value::as_str)
                && !is_colour(c)
            {
                v.push(format!("image {i} ROI colour {c:?} is not #RRGGBB"));
            }
        }
    }
    check_frames(im, v);
}

/// Per-frame records (`extra.frames`, attached like `info --view full` does) use the shared
/// vocabulary: integer `t`/`z`/`c` inside the image's axes, finite times and positions.
fn check_frames(im: &ImageInfo, v: &mut Vec<String>) {
    let Some(frames) = im.extra.get("frames").and_then(Value::as_array) else {
        return;
    };
    let i = im.index;
    for (n, r) in frames.iter().enumerate() {
        let Some(o) = r.as_object() else {
            v.push(format!("image {i} frame record {n} is not an object"));
            continue;
        };
        for (k, size) in [("t", im.size_t), ("z", im.size_z), ("c", im.size_c)] {
            if let Some(x) = o.get(k)
                && x.as_u64().is_none_or(|x| x >= u64::from(size))
            {
                v.push(format!("image {i} frame {n}: {k} = {x} outside 0..{size}"));
            }
        }
        for k in [
            "time_ms",
            "delta_t_s",
            "stage_x_um",
            "stage_y_um",
            "stage_z_um",
            "exposure_ms",
        ] {
            if let Some(x) = o.get(k)
                && !x.as_f64().is_some_and(f64::is_finite)
            {
                v.push(format!(
                    "image {i} frame {n}: {k} = {x} is not a finite number"
                ));
            }
        }
        if let Some(e) = o.get("exposure_ms").and_then(Value::as_f64)
            && e < 0.0
        {
            v.push(format!("image {i} frame {n}: negative exposure {e}"));
        }
        if let Some(a) = o.get("acquired_at").and_then(Value::as_str)
            && !matches!(iso8601(a), Ok(true))
        {
            v.push(format!(
                "image {i} frame {n}: acquired_at {a:?} is not ISO-8601 UTC"
            ));
        }
    }
}

// ---------- tables, traces, spectra ----------

const DTYPES: &[&str] = &[
    "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float32", "float64",
];

/// "Missing is omitted" (book/src/guides/metadata.md): no normalized text field outside
/// `extra` (the format's own vocabulary) and `notes` is an empty or blank string. Column and
/// signal-channel names are required strings and may legitimately be empty in a file
/// (`name: ""` is what the file says), so arrays of them are checked by their readers instead.
fn check_no_empty_text(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "extra" || k == "notes" || k == "vendor" {
                    continue;
                }
                check_no_empty_text(x, &format!("{path}.{k}"), out);
            }
        }
        Value::Array(a) => {
            for x in a {
                check_no_empty_text(x, &format!("{path}[]"), out);
            }
        }
        Value::String(s) if s.trim().is_empty() => {
            let required_name = path.ends_with(".columns[].name")
                || (path.ends_with(".channels[].name") && path.contains("traces"));
            if !required_name {
                out.push(format!("{path} is an empty string (omit it instead)"));
            }
        }
        _ => {}
    }
}

fn check_file(info: &FileInfo, v: &mut Vec<String>) {
    if let Ok(j) = serde_json::to_value(info) {
        check_no_empty_text(&j, "info", v);
    }
    let zone_note = info.notes.iter().any(|n| n.contains("time zone"));
    for im in &info.images {
        check_image(im, zone_note, v);
    }
    let idx: Vec<u32> = info.images.iter().map(|i| i.index).collect();
    if idx.iter().copied().ne(0..idx.len() as u32) {
        v.push(format!("image indices {idx:?} are not 0..n"));
    }
    let total: u64 = info.images.iter().map(|i| i.plane_count).sum();
    if info.plane_count != total {
        v.push(format!(
            "file plane_count {} != sum over images {total}",
            info.plane_count
        ));
    }
    for t in &info.tables {
        let cols: Vec<u32> = t.columns.iter().map(|c| c.index).collect();
        if cols.iter().copied().ne(0..cols.len() as u32) {
            v.push(format!("table {} column indices are not 0..n", t.index));
        }
        for c in &t.columns {
            if !DTYPES.contains(&c.dtype.as_str()) {
                v.push(format!(
                    "table {} column {} dtype {:?}",
                    t.index, c.name, c.dtype
                ));
            }
            if let Some([a, b]) = c.range
                && a > b
            {
                v.push(format!(
                    "table {} column {} range [{a}, {b}] reversed",
                    t.index, c.name
                ));
            }
        }
        for k in ["acquisition_start", "acquisition_end"] {
            if let Some(a) = t.extra.get(k).and_then(Value::as_str) {
                check_time(&format!("table {} {k}", t.index), a, zone_note, v);
            }
        }
        if let Some(d) = t.extra.get("acquisition_date").and_then(Value::as_str)
            && iso8601(&format!("{d}T00:00:00Z")).is_err()
        {
            v.push(format!(
                "table {} acquisition_date {d:?} is not YYYY-MM-DD",
                t.index
            ));
        }
    }
    for t in &info.traces {
        // Irregularly sampled traces (chromatograms) have rate 0 and a `time` channel.
        let irregular = t.sample_rate_hz == 0.0
            && t.extra.contains_key("irregular_sampling")
            && t.channels.first().is_some_and(|c| c.name == "time");
        let rate_ok = t.sample_rate_hz.is_finite() && t.sample_rate_hz > 0.0;
        // Traces over a non-time abscissa (qPCR cycles, melt temperature) have rate 0 and name
        // their axis in `extra.axis`.
        let other_axis = t.sample_rate_hz == 0.0
            && t.extra
                .get("axis")
                .and_then(|a| a.get("quantity"))
                .and_then(|q| q.as_str())
                .is_some_and(|q| q != "time");
        if !(irregular || rate_ok || other_axis) {
            v.push(format!(
                "trace {} sample_rate_hz {}",
                t.index, t.sample_rate_hz
            ));
        }
        let ch: Vec<u32> = t.channels.iter().map(|c| c.index).collect();
        if ch.iter().copied().ne(0..ch.len() as u32) {
            v.push(format!("trace {} channel indices are not 0..n", t.index));
        }
    }
    for s in &info.spectra {
        if let Some([a, b]) = s.rt_range_s
            && !(a.is_finite() && b.is_finite() && 0.0 <= a && a <= b)
        {
            v.push(format!("spectra {} rt_range_s [{a}, {b}]", s.index));
        }
        if s.ms_levels.contains(&0) {
            v.push(format!("spectra {} has MS level 0", s.index));
        }
    }
}

// ---------- provenance keys ----------

#[derive(Debug, Clone)]
enum Sel {
    Any,
    Index(usize),
    Eq(String, String),
}

#[derive(Debug, Clone)]
struct Seg {
    name: String,
    sels: Vec<Sel>,
}

/// `images[].channels[0].name`, `tables[].extra.spillover[keyword=SPILL]` → segments.
fn parse_key(key: &str) -> Result<Vec<Seg>, String> {
    let mut out = Vec::new();
    for part in key.split('.') {
        let (name, mut rest) = part.split_at(part.find('[').unwrap_or(part.len()));
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("segment {part:?} has no field name"));
        }
        let mut sels = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let end = r
                .find(']')
                .ok_or_else(|| format!("unclosed [ in {part:?}"))?;
            let inner = &r[..end];
            sels.push(if inner.is_empty() {
                Sel::Any
            } else if let Ok(n) = inner.parse() {
                Sel::Index(n)
            } else if let Some((k, v)) = inner.split_once('=') {
                Sel::Eq(k.into(), v.into())
            } else {
                return Err(format!("bad selector [{inner}]"));
            });
            rest = &r[end + 1..];
        }
        if !rest.is_empty() {
            return Err(format!("trailing {rest:?} in {part:?}"));
        }
        out.push(Seg {
            name: name.into(),
            sels,
        });
    }
    Ok(out)
}

/// Candidate schemas after resolving `$ref` and non-null `anyOf`/`oneOf` alternatives.
fn alternatives<'a>(s: &'a Value, root: &'a Value) -> Vec<&'a Value> {
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or_default();
        return root
            .get("$defs")
            .and_then(|d| d.get(name))
            .map(|d| alternatives(d, root))
            .unwrap_or_default();
    }
    for k in ["anyOf", "oneOf", "allOf"] {
        if let Some(a) = s.get(k).and_then(Value::as_array) {
            return a.iter().flat_map(|x| alternatives(x, root)).collect();
        }
    }
    vec![s]
}

/// Does the model schema have this path? Anything under an `extra` map (free-form, our own
/// per-format vocabulary) is accepted.
fn schema_has(schema: &Value, root: &Value, segs: &[Seg]) -> bool {
    let Some((seg, rest)) = segs.split_first() else {
        return true;
    };
    alternatives(schema, root).into_iter().any(|s| {
        let Some(mut field) = s.get("properties").and_then(|p| p.get(&seg.name)) else {
            return false;
        };
        if seg.name == "extra" {
            return true;
        }
        let mut items_ok = true;
        for sel in &seg.sels {
            if matches!(sel, Sel::Eq(..)) {
                continue; // a filter on the same value
            }
            match alternatives(field, root)
                .into_iter()
                .find_map(|a| a.get("items"))
            {
                Some(i) => field = i,
                None => items_ok = false,
            }
        }
        items_ok && schema_has(field, root, rest)
    })
}

/// Does `v` have this path (for at least one element where `[]` is used)?
fn value_has(v: &Value, segs: &[Seg]) -> bool {
    let Some((seg, rest)) = segs.split_first() else {
        return !v.is_null();
    };
    let Some(mut cur) = v.get(&seg.name).map(|x| vec![x]) else {
        return false;
    };
    for sel in &seg.sels {
        cur = cur
            .into_iter()
            .flat_map(|c| -> Vec<&Value> {
                match (sel, c) {
                    (Sel::Any, Value::Array(a)) => a.iter().collect(),
                    (Sel::Index(n), Value::Array(a)) => a.get(*n).into_iter().collect(),
                    (Sel::Eq(k, val), Value::Array(a)) => a
                        .iter()
                        .filter(|e| e.get(k).and_then(Value::as_str) == Some(val))
                        .collect(),
                    (Sel::Eq(k, val), o) if o.get(k).and_then(Value::as_str) == Some(val) => {
                        vec![o]
                    }
                    _ => Vec::new(),
                }
            })
            .collect();
    }
    cur.into_iter().any(|c| value_has(c, rest))
}

// ---------- the walk ----------

#[test]
fn corpus_metadata_conforms() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let schema = serde_json::to_value(schemars::schema_for!(FileInfo)).unwrap();

    let mut checked = 0usize;
    let mut failures: Vec<(String, Vec<String>)> = Vec::new();
    // format → provenance key → realized in at least one file?
    let mut realized: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    let mut per_format: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    // Formats with a development file missing here (e.g. CI's smoke tier): a provenance key no
    // present file fills may be filled by an absent one, so the dead-key check skips them.
    let mut incomplete: BTreeSet<String> = BTreeSet::new();
    for e in manifest
        .file
        .iter()
        .filter(|e| e.role.is_empty() || e.role == "input")
    {
        if only.as_ref().is_some_and(|o| !e.id.contains(o.as_str())) {
            continue;
        }
        let path = files_dir.join(&e.filename);
        if !path.exists() {
            incomplete.insert(e.format.clone());
            continue;
        }
        let ds = match reg.open(&path) {
            Ok((_, ds)) => ds,
            // Formats without a reader on this branch, and files a reader declines on purpose.
            Err(Error::UnknownFormat { .. } | Error::Unsupported { .. }) => continue,
            Err(err) => {
                failures.push((e.id.clone(), vec![format!("open: {err}")]));
                continue;
            }
        };
        let mut info = match ds.info() {
            Ok(i) => i,
            Err(Error::Unsupported { .. }) => continue,
            Err(err) => {
                failures.push((e.id.clone(), vec![format!("info: {err}")]));
                continue;
            }
        };
        let mut v = Vec::new();
        if let Err(err) = openreadout_core::reader::attach_frames(ds.as_ref(), &mut info, Some(100))
        {
            v.push(format!("frames: {err}"));
        }
        check_file(&info, &mut v);
        let json = serde_json::to_value(&info).unwrap();
        let fmt = info.format.id.clone();
        let seen = realized.entry(fmt.clone()).or_default();
        for key in ds.provenance().keys() {
            match parse_key(key) {
                Ok(segs) => {
                    if !schema_has(&schema, &schema, &segs) {
                        v.push(format!(
                            "provenance key {key:?} names no field of the model"
                        ));
                    }
                    *seen.entry(key.clone()).or_insert(false) |= value_has(&json, &segs);
                }
                Err(err) => v.push(format!("provenance key {key:?}: {err}")),
            }
        }
        checked += 1;
        let stat = per_format.entry(fmt).or_default();
        stat.0 += 1;
        if v.is_empty() {
            println!("pass   {:<5} {}", e.format, e.id);
        } else {
            stat.1 += 1;
            println!("FAIL   {:<5} {}", e.format, e.id);
            for x in &v {
                println!("         {x}");
            }
            failures.push((e.id.clone(), v));
        }
    }
    // Provenance keys that never resolve anywhere are typos or dead entries.
    let allowed: BTreeSet<(&str, &str)> = UNREALIZED_OK.iter().map(|(f, k, _)| (*f, *k)).collect();
    for (fmt, keys) in realized
        .iter()
        .filter(|(f, _)| only.is_none() && !incomplete.contains(*f))
    {
        let dead: Vec<&String> = keys
            .iter()
            .filter(|(k, ok)| !**ok && !allowed.contains(&(fmt.as_str(), k.as_str())))
            .map(|(k, _)| k)
            .collect();
        if !dead.is_empty() {
            failures.push((
                format!("{fmt} provenance"),
                dead.iter()
                    .map(|k| format!("key {k:?} is filled by no corpus file (add it to UNREALIZED_OK with a reason, or fix it)"))
                    .collect(),
            ));
        }
    }
    println!("\nformat  files  failing");
    for (f, (n, bad)) in &per_format {
        println!("{f:<7} {n:>5}  {bad:>7}");
    }
    assert!(
        checked > 0,
        "no corpus files found; run `cargo xtask corpus fetch --tier smoke`"
    );
    assert!(
        failures.is_empty(),
        "{} metadata conformance failures:\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(id, v)| format!("  {id}:\n    {}", v.join("\n    ")))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Every reader, in the CLI's detection order (`support/registry.rs`, kept equal to
/// `crates/openreadout-cli/src/registry.rs` by `openreadout`'s `tests/registry_parity.rs`).
fn full_registry() -> Registry {
    shared_registry::registry()
}

/// Problems with one file's experiment (`book/src/guides/metadata.md` § Conformance).
fn check_experiment(e: &Experiment, v: &mut Vec<String>) {
    for t in e.terms() {
        if !vocab::is_known(t) {
            v.push(format!(
                "term {} “{}” is not in the curated table",
                t.id, t.label
            ));
        }
    }
    for (path, q) in e.quantities() {
        match &q.unit {
            Some(u) => match vocab::ucum(u) {
                Some(code) if q.ucum.as_deref() == Some(code) => {}
                _ => v.push(format!(
                    "{path}: unit {u:?} has no (or the wrong) UCUM code {:?}",
                    q.ucum
                )),
            },
            None if q.ucum.is_some() => v.push(format!("{path}: UCUM code without a unit")),
            None => {}
        }
        if q.value.is_null() {
            v.push(format!("{path}: null value"));
        }
    }
    for p in e.value_paths() {
        if e.origin_of(&p).is_none() {
            v.push(format!("{p} has no provenance"));
        }
    }
    for k in e.provenance.keys() {
        if !e.value_paths().iter().any(|p| p == k) {
            v.push(format!("provenance key {k} names no value"));
        }
    }
    if let Some(s) = &e.sample {
        if s.id.is_some() && s.source_field.is_none() {
            v.push("sample.id without sample.source_field".into());
        }
        for (k, x) in [
            ("id", &s.id),
            ("name", &s.name),
            ("well", &s.well),
            ("barcode", &s.barcode),
            ("sequence_position", &s.sequence_position),
        ] {
            if x.as_deref()
                .is_some_and(|x| x.trim().is_empty() || x.trim() != x)
            {
                v.push(format!("sample.{k} {x:?} is empty or padded"));
            }
        }
    }
    if let Some(a) = &e.acquisition {
        for (k, t) in [("started_at", &a.started_at), ("ended_at", &a.ended_at)] {
            if let Some(t) = t
                && iso8601(t).is_err()
            {
                v.push(format!("acquisition.{k} {t:?} is not ISO-8601"));
            }
        }
        if let Some(d) = a.duration_s
            && !(d.is_finite() && d > 0.0)
        {
            v.push(format!(
                "acquisition.duration_s {d} is not a positive duration"
            ));
        }
    }
    for (i, m) in e.measurements.iter().enumerate() {
        if m.what.trim().is_empty() || m.indices.is_empty() {
            v.push(format!(
                "measurements[{i}] has no description or no indices"
            ));
        }
    }
}

/// Experiment fields whose fill rate the walk prints.
const FIELDS: [&str; 6] = [
    "sample.id",
    "instrument.model",
    "method.name",
    "method.technique",
    "acquisition.operator",
    "acquisition.started_at",
];

/// Columns of the per-format fill-rate table this test prints: a file counts when
/// any of the paths (or, for a trailing `.`, any path under it) has a value.
const TABLE: [(&str, &[&str]); 13] = [
    ("sample id", &["sample.id"]),
    (
        "well/vial/barcode",
        &["sample.well", "sample.sequence_position", "sample.barcode"],
    ),
    ("vendor", &["instrument.vendor"]),
    ("model", &["instrument.model"]),
    ("serial", &["instrument.serial"]),
    ("software", &["instrument.software"]),
    ("method name", &["method.name"]),
    ("technique", &["method.technique"]),
    ("parameters", &["method.parameters."]),
    ("start", &["acquisition.started_at"]),
    ("operator", &["acquisition.operator"]),
    ("duration", &["acquisition.duration_s"]),
    ("comment", &["acquisition.comment"]),
];

fn table_hit(paths: &[String], want: &[&str]) -> bool {
    want.iter().any(|w| {
        paths.iter().any(|p| {
            if w.ends_with('.') {
                p.starts_with(w)
            } else {
                p == w
            }
        })
    })
}

/// `all`, `—` or `hits/n`.
fn table_cell(hits: usize, n: usize) -> String {
    match hits {
        0 => "—".into(),
        h if h == n => "all".into(),
        h => format!("{h}/{n}"),
    }
}

/// Right-aligned fractions `hits / n`, one 12-wide column each.
fn rate_columns(hits: &[usize], n: usize) -> String {
    let mut s = String::new();
    for h in hits {
        let _ = write!(s, "{:>12.2}", *h as f64 / n as f64);
    }
    s
}

/// The `FIELDS` header, one 12-wide column each.
fn field_columns() -> String {
    let mut s = String::new();
    for f in FIELDS {
        let _ = write!(s, "{:>12}", f.rsplit('.').next().unwrap_or(f));
    }
    s
}

#[test]
fn corpus_experiments_are_well_formed() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = full_registry();
    // family → (files, per-field hits)
    let mut fill: BTreeMap<String, (usize, [usize; 6])> = BTreeMap::new();
    // format id → (files, per-column hits)
    let mut by_format: BTreeMap<String, (usize, [usize; 13])> = BTreeMap::new();
    let mut terms: BTreeSet<String> = BTreeSet::new();
    let mut failures: Vec<(String, Vec<String>)> = Vec::new();
    for e in manifest
        .file
        .iter()
        .filter(|e| e.role.is_empty() || e.role == "input")
    {
        if only.as_ref().is_some_and(|o| !e.id.contains(o.as_str())) {
            continue;
        }
        let path = files_dir.join(&e.filename);
        if !path.exists() {
            continue;
        }
        let Ok((_, ds)) = reg.open(&path) else {
            continue;
        };
        let Ok(info) = ds.info() else { continue };
        let exp = experiment::of_dataset(ds.as_ref(), &info);
        let mut v = Vec::new();
        check_experiment(&exp, &mut v);
        let paths = exp.value_paths();
        let stat = fill.entry(info.format.family.clone()).or_default();
        stat.0 += 1;
        for (n, f) in FIELDS.iter().enumerate() {
            if paths.iter().any(|p| p == f) {
                stat.1[n] += 1;
            }
        }
        let row = by_format.entry(info.format.id.clone()).or_default();
        row.0 += 1;
        for (n, (_, want)) in TABLE.iter().enumerate() {
            if table_hit(&paths, want) {
                row.1[n] += 1;
            }
        }
        terms.extend(exp.terms().iter().map(|t| t.id.clone()));
        if !v.is_empty() {
            failures.push((e.id.clone(), v));
        }
    }
    println!("\nexperiment fill rates (fraction of files)");
    println!("{:<22} {:>5} {}", "family", "files", field_columns());
    let mut total = (0usize, [0usize; 6]);
    for (f, (n, hits)) in &fill {
        total.0 += n;
        for (t, h) in total.1.iter_mut().zip(hits) {
            *t += h;
        }
        println!("{f:<22} {n:>5} {}", rate_columns(hits, *n));
    }
    if total.0 > 0 {
        println!(
            "{:<22} {:>5} {}",
            "all",
            total.0,
            rate_columns(&total.1, total.0)
        );
    }
    println!("\nexperiment fill rates per format\n");
    println!(
        "| format | files | {} |",
        TABLE
            .iter()
            .map(|(h, _)| *h)
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!("| --- | ---: |{}", " ---: |".repeat(TABLE.len()));
    for (f, (n, hits)) in &by_format {
        println!(
            "| {f} | {n} | {} |",
            hits.iter()
                .map(|h| table_cell(*h, *n))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    println!(
        "{} distinct terms emitted of {} curated",
        terms.len(),
        vocab::TERMS.len()
    );
    assert!(total.0 > 0, "no corpus files found");
    assert!(
        failures.is_empty(),
        "{} experiment conformance failures:\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(id, v)| format!("  {id}:\n    {}", v.join("\n    ")))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn helpers() {
    assert_eq!(iso8601("2017-06-06T09:15:06.980Z"), Ok(true));
    assert_eq!(iso8601("2017-06-06T09:15:06+02:00"), Ok(true));
    assert_eq!(iso8601("2014-07-18T09:44:26"), Ok(false));
    assert!(iso8601("06/06/2017 11:15").is_err());
    assert!(iso8601("2017-13-06T09:15:06Z").is_err());
    assert!(is_colour("#00FF7F"));
    assert!(!is_colour("#00ff7f"));
    let schema = serde_json::to_value(schemars::schema_for!(FileInfo)).unwrap();
    let ok = |k: &str| schema_has(&schema, &schema, &parse_key(k).unwrap());
    assert!(ok("images[].physical_size.x"));
    assert!(ok("images[].channels[].emission_range_nm"));
    assert!(ok("images[].extra.anything"));
    assert!(ok("tables[].extra.spillover[keyword=SPILL]"));
    assert!(ok("images[].objective"));
    assert!(!ok("images[].pixel_size"));
    assert!(!ok("images[].channels[].wavelength"));
    let v = serde_json::json!({"tables": [{"extra": {"spillover": {"keyword": "SPILL"}}}]});
    assert!(value_has(
        &v,
        &parse_key("tables[].extra.spillover[keyword=SPILL]").unwrap()
    ));
    assert!(!value_has(
        &v,
        &parse_key("tables[].extra.spillover[keyword=SPILLOVER]").unwrap()
    ));
}
