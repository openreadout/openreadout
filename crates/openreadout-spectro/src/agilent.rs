//! Agilent FT-IR imaging files (focal-plane-array microscopes: Cary 600 series with Resolutions
//! Pro): a data file of band-sequential float32 images (`.dat` spectra, `.seq` interferograms of
//! one tile; `.dms` a whole mosaic, `.dmd`/`.drd` one mosaic tile) and a compound-file header
//! beside it (`.bsp` for one tile, `.dmt` for a mosaic) holding the axis and the settings.
//!
//! Layout and vocabulary: `docs/formats/agilent-fpa.md`; provenance:
//! `docs/provenance/agilent-fpa.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{find, le_u32};
use openreadout_core::cfb::{CFB_MAGIC, Cfb};
use openreadout_core::model::LsEntry;
use openreadout_core::provenance::Source;
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::common::{Facts, Le, MapSpec, Parsed, Rows, SpectrumSet, Stored, XValues, num, read_at};

const FMT: &str = crate::AGILENT_FPA_FORMAT_ID;
/// First four bytes of every data file.
pub(crate) const DATA_MAGIC: [u8; 4] = [0x00, 0x72, 0x47, 0x00];
/// Bytes before the first value of a data file (255 float32 slots).
const DATA_HEADER: u64 = 1020;
/// Largest header stream read.
const MAX_STREAM: u64 = 16 << 20;
/// Data-file extensions: spectra of one tile, interferograms of one tile, a whole mosaic, a
/// mosaic tile's spectra, a mosaic tile's interferograms.
pub(crate) const DATA_EXTENSIONS: [&str; 5] = ["dat", "seq", "dms", "dmd", "drd"];
/// Header-file extensions.
pub(crate) const HEADER_EXTENSIONS: [&str; 2] = ["bsp", "dmt"];

/// The data file's own header.
#[derive(Debug, Clone)]
struct DataHeader {
    points: u32,
    width: u32,
    height: u32,
    version: String,
    aggregation: u32,
}

fn data_header(b: &[u8]) -> Option<DataHeader> {
    if b.get(..4)? != DATA_MAGIC {
        return None;
    }
    let points = b.u32_at(9)?;
    let width = u32::from(b.u16_at(24)?);
    let height = u32::from(b.u16_at(26)?);
    let version = b
        .bytes_at(129, 16)
        .map(crate::common::text_field)
        .unwrap_or_default();
    let aggregation = b.u32_at(170)?;
    (points > 0 && width > 0 && height > 0).then_some(DataHeader {
        points,
        width,
        height,
        version,
        aggregation,
    })
}

/// True when `head` looks like a data file: the magic, plausible sizes and a version text of
/// digits and dots.
pub(crate) fn looks_like_data(head: &[u8]) -> bool {
    head.len() >= 180
        && data_header(head).is_some_and(|h| {
            !h.version.is_empty() && h.version.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

/// What the header stream says.
#[derive(Debug, Default)]
struct Header {
    stream: String,
    /// Spacing, first index and point count of the spectral axis (`Data` record).
    axis: Option<(f64, i64, u64)>,
    /// The same for the interferogram axis (`Interferogram` property).
    ifg_axis: Option<(f64, i64, u64)>,
    labels: Option<(String, String)>,
    /// The one pixel whose spectrum the header holds (`Row = r Col = c`).
    selected: Option<(u32, u32)>,
    settings: Vec<(String, String)>,
    numbers: Vec<(String, f64)>,
    problems: Vec<String>,
}

impl Header {
    fn setting(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(k, v)| k == key && !v.trim().is_empty())
            .map(|(_, v)| v.trim())
    }
    fn setting_num(&self, key: &str) -> Option<f64> {
        self.setting(key)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())
    }
    fn number(&self, key: &str) -> Option<f64> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

/// A `Data` record starting at the text `Data` (index `at`): spacing, first index, points.
fn data_record(d: &[u8], at: usize) -> Option<(f64, i64, u64)> {
    // "Data", u32 0, u32 1, u32 4, "1.00", u32 1, u32 4, u32 8, f64, u32 4, i32, u32 4, i32
    if d.get(at..at + 4)? != b"Data" || le_u32(d, at.checked_sub(4)?)? != 4 {
        return None;
    }
    let p = at + 4;
    if [le_u32(d, p)?, le_u32(d, p + 4)?, le_u32(d, p + 8)?] != [0, 1, 4]
        || d.get(p + 12..p + 16)? != b"1.00"
    {
        return None;
    }
    let q = p + 16;
    if [le_u32(d, q)?, le_u32(d, q + 4)?, le_u32(d, q + 8)?] != [1, 4, 8] {
        return None;
    }
    let spacing = d.f64_at(q + 12)?;
    if le_u32(d, q + 20)? != 4 || le_u32(d, q + 28)? != 4 {
        return None;
    }
    let first = i64::from(d.i32_at(q + 24)?);
    let n = u64::try_from(d.i32_at(q + 32)?).ok()?;
    (spacing.is_finite() && spacing != 0.0 && n > 0).then_some((spacing, first, n))
}

/// A doubled-length text (u32 4, u32 n, u32 n, n bytes) at `at`: the text and the next index.
fn doubled_text(d: &[u8], at: usize) -> Option<(String, usize)> {
    if le_u32(d, at)? != 4 {
        return None;
    }
    let n = le_u32(d, at + 4)? as usize;
    if le_u32(d, at + 8)? as usize != n || n > 4096 {
        return None;
    }
    let b = d.get(at + 12..at + 12 + n)?;
    Some((b.iter().map(|&c| char::from(c)).collect(), at + 12 + n))
}

fn printable(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| (' '..='~').contains(&c))
}

/// Parse the header stream.
fn parse_stream(d: &[u8], name: &str) -> Header {
    let mut h = Header {
        stream: name.to_string(),
        ..Header::default()
    };
    // the image's own Data record is the first one
    if let Some(at) = d.windows(4).position(|w| w == b"Data") {
        h.axis = data_record(d, at);
    }
    if h.axis.is_none() {
        h.problems
            .push("the header's spectral axis record (`Data`) was not found".into());
    }
    // the interferogram axis: `Interferogram`, a PropType record of code 7, a nested Data record
    if let Some(at) = find(d, b"Interferogram") {
        let from = at + 13;
        if let Some(k) = find(&d[from..d.len().min(from + 96)], b"Data") {
            h.ifg_axis = data_record(d, from + k);
        }
    }
    // x and y labels after the `Parms` record's `XO` code
    if let Some(p) = find(d, b"Parms")
        && let Some(xo) = find(&d[p..d.len().min(p + 256)], b"XO\x00\x00")
    {
        let at = p + xo + 4;
        if let Some((x, next)) = doubled_text(d, at)
            && let Some((y, _)) = doubled_text(d, next)
            && printable(&x)
            && printable(&y)
        {
            h.labels = Some((x, y));
        }
    }
    // the selected pixel: `Row = r Col = c`
    if let Some(at) = find(d, b"Row = ") {
        let text: String = d[at..d.len().min(at + 40)]
            .iter()
            .take_while(|c| c.is_ascii_graphic() || **c == b' ')
            .map(|&c| char::from(c))
            .collect();
        let nums: Vec<u32> = text
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|t| t.parse().ok())
            .collect();
        if let [r, c, ..] = nums.as_slice() {
            h.selected = Some((*r, *c));
        }
    }
    // string settings: key (doubled text), u32 4 1 4 2 4 4 4, u32 n, u32 4, u32 n, u32 n, text
    let mut i = 0usize;
    while i + 12 < d.len() {
        if let Some((key, j)) = doubled_text(d, i)
            && printable(&key)
            && (0..7)
                .map(|k| le_u32(d, j + 4 * k))
                .collect::<Option<Vec<_>>>()
                == Some(vec![4, 1, 4, 2, 4, 4, 4])
            && let Some(n) = le_u32(d, j + 28)
            && le_u32(d, j + 32) == Some(4)
            && le_u32(d, j + 36) == Some(n)
            && le_u32(d, j + 40) == Some(n)
            && let Some(v) = d.get(j + 44..j + 44 + n as usize)
        {
            h.settings
                .push((key, v.iter().map(|&c| char::from(c)).collect()));
            i = j + 44 + n as usize;
            continue;
        }
        i += 1;
    }
    // numeric properties: u32 n, key, u32 0, u32 2, u32 8, "PropType", u32 1, u32 1, u32 4,
    // u32 0 (a number), u32 4, "1.00", u32 1, u32 1, u32 8, f64
    let mut from = 0usize;
    while let Some(k) = find(&d[from..], b"PropType") {
        let at = from + k;
        from = at + 8;
        if at < 16 {
            continue;
        }
        let key_end = at - 12;
        if [
            le_u32(d, key_end),
            le_u32(d, key_end + 4),
            le_u32(d, key_end + 8),
        ] != [Some(0), Some(2), Some(8)]
        {
            continue;
        }
        let Some(key) = (1..=64usize).find_map(|n| {
            let s = key_end.checked_sub(n)?;
            let len = le_u32(d, s.checked_sub(4)?)? as usize;
            let b = d.get(s..key_end)?;
            (len == n && b.iter().all(|c| (0x20..0x7f).contains(c)))
                .then(|| b.iter().map(|&c| char::from(c)).collect::<String>())
        }) else {
            continue;
        };
        let q = at + 8;
        if [
            le_u32(d, q),
            le_u32(d, q + 4),
            le_u32(d, q + 8),
            le_u32(d, q + 12),
        ] != [Some(1), Some(1), Some(4), Some(0)]
            || le_u32(d, q + 16) != Some(4)
            || d.get(q + 20..q + 24) != Some(b"1.00")
            || [le_u32(d, q + 24), le_u32(d, q + 28), le_u32(d, q + 32)]
                != [Some(1), Some(1), Some(8)]
        {
            continue;
        }
        if let Some(v) = d.f64_at(q + 36).filter(|v| v.is_finite()) {
            h.numbers.push((key, v));
        }
    }
    h
}

/// The header file of a data file (or the data file of a header file), found beside it.
fn sibling(input: &Input, candidates: &[String]) -> Option<PathBuf> {
    let dir = input.path().parent().unwrap_or_else(|| Path::new(""));
    candidates.iter().find_map(|name| {
        let p = dir.join(name);
        if input.fs().is_file(&p) {
            return Some(p);
        }
        input.fs().find_in_dir(dir, name)
    })
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn stem_of(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// A mosaic tile's stem without its `_XXXX_YYYY` suffix.
fn mosaic_stem(stem: &str) -> Option<(&str, u32, u32)> {
    let (rest, y) = stem.rsplit_once('_')?;
    let (base, x) = rest.rsplit_once('_')?;
    (x.len() == 4 && y.len() == 4)
        .then(|| Some((base, x.parse().ok()?, y.parse().ok()?)))
        .flatten()
}

/// The data file to read for `input`: itself, or the data file of a header file.
pub(crate) fn resolve(input: &Input) -> Result<Input> {
    let ext = ext_of(input.path());
    if !HEADER_EXTENSIONS.contains(&ext.as_str()) {
        return Ok(input.clone());
    }
    let stem = stem_of(input.path());
    let names: Vec<String> = if ext == "bsp" {
        ["dat", "DAT", "seq", "SEQ"]
            .iter()
            .map(|e| format!("{stem}.{e}"))
            .collect()
    } else {
        ["dms", "DMS"]
            .iter()
            .map(|e| format!("{stem}.{e}"))
            .collect()
    };
    let p = sibling(input, &names).ok_or_else(|| {
        Error::unsupported(
            FMT,
            format!(
                "{}: no data file beside the header ({})",
                input.path().display(),
                names.join(", ")
            ),
            "Open the data file (.dat or .seq for one tile, .dms for a whole mosaic, .dmd/.drd for one mosaic tile); the header alone holds no image.",
        )
    })?;
    Ok(input.with_path(p))
}

/// Opens a data file (after [`resolve`]) with its header.
pub(crate) fn parse(
    f: &openreadout_core::source::SourceFile,
    input: &Input,
    file_len: u64,
) -> Result<Parsed> {
    let path = input.path();
    let head = read_at(f, path, 0, DATA_HEADER, file_len)?;
    let Some(dh) = data_header(&head) else {
        return Err(Error::corrupt(
            FMT,
            "not an Agilent FT-IR imaging data file (no 00 72 47 00 header)",
        ));
    };
    let ext = ext_of(path);
    let ifg = matches!(ext.as_str(), "seq" | "drd");
    let want = u64::from(dh.points)
        .checked_mul(u64::from(dh.width))
        .and_then(|v| v.checked_mul(u64::from(dh.height)))
        .and_then(|v| v.checked_mul(4))
        .and_then(|v| v.checked_add(DATA_HEADER))
        .ok_or_else(|| Error::corrupt(FMT, "image size overflows"))?;
    if want != file_len {
        return Err(Error::corrupt(
            FMT,
            format!(
                "{} points of {} × {} pixels need {want} bytes; the file has {file_len}",
                dh.points, dh.width, dh.height
            ),
        ));
    }
    let mut parsed = Parsed {
        format_version: (!dh.version.is_empty()).then(|| dh.version.clone()),
        ..Parsed::default()
    };
    let stem = stem_of(path);
    let tile = if matches!(ext.as_str(), "dmd" | "drd") {
        mosaic_stem(&stem)
    } else {
        None
    };
    let (header, header_path) = find_header(input, &ext, &stem, tile, &mut parsed);
    let mosaic_header = header_path.as_ref().is_some_and(|p| ext_of(p) == "dmt");
    parsed
        .notes
        .push(kind_note(&ext, mosaic_header).to_string());

    let n = u64::from(dh.points);
    let (w, h) = (dh.width, dh.height);
    let count = u64::from(w) * u64::from(h);
    let mut extra = BTreeMap::new();
    let (xq, xu, x) = spectral_axis(&header, ifg, n, &mut extra, &mut parsed);
    let (y_name, y_unit) = if ifg {
        ("interferogram".to_string(), None)
    } else {
        match header.labels.as_ref().map(|(_, y)| y.as_str()) {
            Some("Absorbance") => ("absorbance".to_string(), None),
            Some("Response") => ("single_beam".to_string(), None),
            Some("Transmittance") => ("transmittance".to_string(), Some("%".to_string())),
            Some("Reflectance") => ("reflectance".to_string(), Some("%".to_string())),
            _ => ("intensity".to_string(), None),
        }
    };
    if let Some((xl, yl)) = &header.labels {
        extra.insert("x_label".into(), json!(xl));
        extra.insert("y_label".into(), json!(yl));
    }
    extra.insert("size_x".into(), json!(w));
    extra.insert("size_y".into(), json!(h));
    extra.insert("aggregation".into(), json!(dh.aggregation));
    if let Some((r, c)) = header.selected {
        extra.insert(
            "header_spectrum_pixel".into(),
            json!({"stored_row": r, "column": c}),
        );
    }
    if let Some((_, x, y)) = tile {
        extra.insert("mosaic_tile".into(), json!([x, y]));
    }
    let fpa = header.number("FPA Pixel Size");
    let pixel_um = fpa
        .filter(|v| *v > 0.0)
        .map(|v| v * f64::from(dh.aggregation.max(1)));
    if let Some(v) = fpa {
        extra.insert("detector_pixel_um".into(), num(v));
    }
    settings_extra(&header, &mut extra);
    let plane = count * 4;
    parsed.sets.push(SpectrumSet {
        name: if ifg {
            "interferograms".into()
        } else {
            format!("{y_name} spectra")
        },
        x_quantity: xq,
        x_unit: xu,
        x,
        y_name,
        y_unit,
        points: n,
        count: u32::try_from(count).unwrap_or(u32::MAX),
        rows: Rows::Interleaved {
            starts: pixel_starts(w, h),
            point_stride: plane,
        },
        stored: Stored::F32,
        scale: 1.0,
        data_type: if ifg {
            "INFRARED INTERFEROGRAM"
        } else {
            "INFRARED SPECTRUM"
        },
        extra,
    });
    let mut mex = BTreeMap::new();
    mex.insert(
        "row_order".into(),
        json!("top to bottom (stored bottom to top)"),
    );
    parsed.maps.push(MapSpec {
        name: stem.clone(),
        set: 0,
        width: w,
        height: h,
        pixels: (0..count).map(|k| u32::try_from(k).ok()).collect(),
        pixel_um: (pixel_um, pixel_um),
        extra: mex,
    });
    parsed.facts = header_facts(&header, pixel_um);
    parsed.vendor = json!({
        "data_file": {"points": dh.points, "width": dh.width, "height": dh.height,
                      "version": dh.version, "aggregation": dh.aggregation},
        "header_stream": header.stream,
        "axis": header.axis.map(|(s, f, n)| json!({"spacing": num(s), "first_index": f, "points": n})),
        "interferogram_axis": header.ifg_axis.map(|(s, f, n)| json!({"spacing": num(s), "first_index": f, "points": n})),
        "labels": header.labels.as_ref().map(|(x, y)| json!([x, y])),
        "settings": header.settings.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "numbers": header.numbers.iter().map(|(k, v)| json!([k, num(*v)])).collect::<Vec<_>>(),
    });
    parsed.entries.push(LsEntry {
        kind: "data".into(),
        name: "header".into(),
        offset: Some(0),
        size: Some(DATA_HEADER),
        image: None,
        details: json!({"points": dh.points, "width": w, "height": h}),
    });
    for c in 0..n.min(4096) {
        parsed.entries.push(LsEntry {
            kind: "plane".into(),
            name: format!("point {c}"),
            offset: Some(DATA_HEADER + c * plane),
            size: Some(plane),
            image: Some(0),
            details: Value::Null,
        });
    }
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.aggregation", Source::Inferred),
        ("images[].size_x", Source::Inferred),
        ("images[].size_y", Source::Inferred),
        ("images[].physical_size", Source::Inferred),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// The header file beside the data file (`.bsp` for one tile, the mosaic's `.dmt` for a
/// mosaic or one of its tiles) and where it was found; problems reading it become notes.
fn find_header(
    input: &Input,
    ext: &str,
    stem: &str,
    tile: Option<(&str, u32, u32)>,
    parsed: &mut Parsed,
) -> (Header, Option<PathBuf>) {
    let names: Vec<String> = match (ext, tile) {
        ("dmd" | "drd", Some((base, _, _))) => {
            vec![
                format!("{base}.dmt"),
                format!("{}.dmt", base.to_lowercase()),
            ]
        }
        ("dms", _) => vec![
            format!("{stem}.dmt"),
            format!("{}.dmt", stem.to_lowercase()),
        ],
        _ => vec![
            format!("{stem}.bsp"),
            format!("{}.bsp", stem.to_lowercase()),
            format!("{stem}.BSP"),
            // a mosaic's whole-image .dat has the mosaic's .dmt
            format!("{stem}.dmt"),
            format!("{}.dmt", stem.to_lowercase()),
        ],
    };
    let mut header = Header::default();
    let mut header_path = None;
    if let Some(hp) = sibling(input, &names) {
        match read_header(&input.with_path(&hp)) {
            Ok(h) => {
                header = h;
                header_path = Some(hp);
            }
            Err(e) => parsed
                .notes
                .push(format!("header {} not read: {e}", hp.display())),
        }
    } else {
        parsed.notes.push(format!(
            "no header file beside the data ({}): the x axis is the point index",
            names.join(" or ")
        ));
    }
    for p in &header.problems {
        parsed.notes.push(format!("header: {p}"));
    }
    (header, header_path)
}

/// What the data file holds, by extension.
fn kind_note(ext: &str, mosaic_header: bool) -> &'static str {
    match ext {
        "dat" if mosaic_header => "Agilent FT-IR mosaic, spectra of the whole mosaic (.dat)",
        "dat" => "Agilent FT-IR image, spectra of one tile (.dat)",
        "seq" => "Agilent FT-IR image, interferograms of one tile (.seq)",
        "dms" => "Agilent FT-IR mosaic, spectra of the whole mosaic (.dms)",
        "dmd" => "Agilent FT-IR mosaic, spectra of one tile (.dmd)",
        _ => "Agilent FT-IR mosaic, interferograms of one tile (.drd)",
    }
}

/// The x axis from the header's axis record when its point count matches the data's,
/// else the point index.
fn spectral_axis(
    header: &Header,
    ifg: bool,
    n: u64,
    extra: &mut BTreeMap<String, Value>,
    parsed: &mut Parsed,
) -> (&'static str, Option<String>, XValues) {
    let axis = if ifg { header.ifg_axis } else { header.axis };
    match axis {
        Some((sp, first, pts)) if pts == n => {
            let (a, b) = (
                sp * first as f64,
                sp * (first + i64::try_from(n - 1).unwrap_or(0)) as f64,
            );
            extra.insert("x_spacing".into(), num(sp));
            extra.insert("x_first_index".into(), json!(first));
            let x = XValues::Regular { first: a, last: b };
            if ifg {
                ("optical_path_difference", Some("cm".to_string()), x)
            } else {
                ("wavenumber", Some("1/cm".to_string()), x)
            }
        }
        other => {
            if let Some((_, _, pts)) = other {
                parsed.notes.push(format!(
                    "the header's axis has {pts} points, the data {n}: the x axis is the point index"
                ));
            }
            let last = n.saturating_sub(1) as f64;
            ("points", None, XValues::Regular { first: 0.0, last })
        }
    }
}

/// The acquisition settings kept on the spectrum set.
fn settings_extra(header: &Header, extra: &mut BTreeMap<String, Value>) {
    for (k, key) in [
        ("resolution_cm1", "Resolution"),
        ("scans", "Scans"),
        ("background_scans", "Background Scans"),
        ("laser_wavenumber_cm1", "Effective Laser Wavenumber"),
        ("undersampling_ratio", "Under Sampling Ratio"),
    ] {
        if let Some(v) = header.setting_num(key) {
            extra.insert(k.into(), num(v));
        }
    }
    for (k, key) in [
        ("apodization", "Apodization Type"),
        ("optics_mode", "OpticsMode"),
        ("objective", "Microscope IR Objective"),
        ("detector", "FPA Camera Type"),
        ("conversion", "To"),
    ] {
        if let Some(v) = header.setting(key) {
            extra.insert(k.into(), json!(v));
        }
    }
}

/// Where each pixel's first value is: sweep k = column + width · row, rows from the top;
/// stored rows run bottom to top.
fn pixel_starts(w: u32, h: u32) -> Vec<u64> {
    (0..u64::from(w) * u64::from(h))
        .map(|k| {
            let col = k % u64::from(w);
            let row_top = k / u64::from(w);
            let stored_row = u64::from(h) - 1 - row_top;
            DATA_HEADER + (stored_row * u64::from(w) + col) * 4
        })
        .collect()
}

fn header_facts(header: &Header, pixel_um: Option<f64>) -> Facts {
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "Agilent", "format");
    if let Some(v) = header.setting("Software Version") {
        Facts::text(&mut facts.software_version, v, "header Software Version");
    }
    if let Some(v) = header.setting("MicroscopePresent") {
        Facts::text(&mut facts.model, v, "header MicroscopePresent");
    }
    if let Some(v) = header.setting("User Stamp") {
        Facts::text(&mut facts.operator, v, "header User Stamp");
    }
    if let Some(v) = header.setting("Sample File Name") {
        Facts::text(&mut facts.sample_name, v, "header Sample File Name");
    }
    if let Some(t) = header.setting("Time Stamp").and_then(time_stamp) {
        Facts::text(&mut facts.started_at, &t, "header Time Stamp (local time)");
    }
    let inf = Source::Inferred;
    let pa = Source::PriorArt;
    for (name, key, unit, src) in [
        ("resolution", "Resolution", Some("1/cm"), pa),
        ("scans", "Scans", None, inf),
        ("background_scans", "Background Scans", None, inf),
        (
            "laser_wavenumber",
            "Effective Laser Wavenumber",
            Some("1/cm"),
            pa,
        ),
        ("integration_time", "Integration Time(ms)", Some("ms"), inf),
    ] {
        if let Some(v) = header.setting_num(key) {
            facts.number(name, v, unit, &format!("header {key}"), src);
        }
    }
    for (name, key) in [
        ("apodization", "Apodization Type"),
        ("optics_mode", "OpticsMode"),
        ("objective", "Microscope IR Objective"),
        ("detector", "FPA Camera Type"),
        ("beamsplitter", "BeamSplitter"),
        ("source", "Source"),
    ] {
        if let Some(v) = header.setting(key) {
            facts.word(name, v, &format!("header {key}"), inf);
        }
    }
    if let Some(v) = pixel_um {
        facts.number(
            "pixel_size",
            v,
            Some("µm"),
            "header FPA Pixel Size × aggregation",
            inf,
        );
    }
    facts
}

/// `Wednesday, July 29, 2020 09:26:49` → `2020-07-29T09:26:49` (local time, no zone).
fn time_stamp(s: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let [_, md, yt] = parts.as_slice() else {
        return None;
    };
    let mut it = md.split_whitespace();
    let month = it.next()?.to_ascii_lowercase();
    let m = MONTHS.iter().position(|x| *x == month)? + 1;
    let d: u32 = it.next()?.parse().ok()?;
    let mut yt = yt.split_whitespace();
    let y: u32 = yt.next()?.parse().ok()?;
    let t: Vec<u32> = yt
        .next()?
        .split(':')
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    let [hh, mm, ss] = t.as_slice() else {
        return None;
    };
    (y >= 1990 && (1..=31).contains(&d) && *hh < 24 && *mm < 60 && *ss <= 60)
        .then(|| format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}"))
}

/// Read the header (compound file) of an image.
fn read_header(input: &Input) -> Result<Header> {
    let mut f = input.open()?;
    let cfb = Cfb::open(&mut f, input.path(), FMT)?;
    let e = cfb
        .entries
        .iter()
        .find(|e| {
            e.is_stream
                && e.path.to_ascii_lowercase().starts_with("spectra/")
                && !e.path.eq_ignore_ascii_case("spectra/indextable")
        })
        .cloned()
        .ok_or_else(|| Error::corrupt(FMT, "the header has no spectrum stream under `Spectra`"))?;
    let d = cfb.read(&mut f, input.path(), &e, MAX_STREAM)?;
    Ok(parse_stream(&d, &e.path))
}

/// True when `input` is a header compound file of an Agilent image (a `Spectra/IndexTable`
/// stream).
pub(crate) fn is_header(head: &[u8], input: &Input) -> bool {
    if !head.starts_with(&CFB_MAGIC) {
        return false;
    }
    let Ok(mut f) = input.open() else {
        return false;
    };
    Cfb::open(&mut f, input.path(), FMT)
        .is_ok_and(|c| c.stream("Spectra/IndexTable").is_some() && c.stream("Version").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
        b.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn header_stream_records() {
        let mut d = Vec::new();
        d.extend_from_slice(&3u32.to_le_bytes());
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(b"Data");
        for v in [0u32, 1, 4] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(b"1.00");
        for v in [1u32, 4, 8] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&1.928_455_114_364_623_8f64.to_le_bytes());
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&466i32.to_le_bytes());
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&1582i32.to_le_bytes());
        // a string setting
        text(&mut d, "Scans");
        for v in [4u32, 1, 4, 2, 4, 4, 4, 2, 4, 2, 2] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(b"64");
        let h = parse_stream(&d, "Spectra/x");
        let (sp, first, n) = h.axis.unwrap();
        assert_eq!((first, n), (466, 1582));
        assert!((sp * 466.0 - 898.66).abs() < 0.01);
        assert_eq!(h.setting_num("Scans"), Some(64.0));
        // truncated anywhere: no panic
        for cut in 0..d.len() {
            let _ = parse_stream(&d[..cut], "x");
        }
    }

    #[test]
    fn data_file_header_and_names() {
        let mut b = vec![0u8; 1020];
        b[..4].copy_from_slice(&DATA_MAGIC);
        b[9..13].copy_from_slice(&1582u32.to_le_bytes());
        b[24..26].copy_from_slice(&32u16.to_le_bytes());
        b[26..28].copy_from_slice(&32u16.to_le_bytes());
        b[129..136].copy_from_slice(b"3.4.0.0");
        b[170] = 1;
        let h = data_header(&b).unwrap();
        assert_eq!(
            (h.points, h.width, h.height, h.aggregation),
            (1582, 32, 32, 1)
        );
        assert_eq!(h.version, "3.4.0.0");
        assert!(looks_like_data(&b));
        b[0] = 1;
        assert!(!looks_like_data(&b));
        assert_eq!(
            mosaic_stem("5_Mosaic_agg1024_0000_0001"),
            Some(("5_Mosaic_agg1024", 0, 1))
        );
        assert_eq!(mosaic_stem("cancer_A"), None);
        assert_eq!(
            time_stamp("Wednesday, July 29, 2020 09:26:49").as_deref(),
            Some("2020-07-29T09:26:49")
        );
    }
}
