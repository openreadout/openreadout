//! Molecular Devices ImageXpress / MetaXpress plates: an `.HTD` plate description and one TIFF
//! per plane named `<plate>_<well>[_s<site>][_w<wave>][<GUID>].tif`, optionally in
//! `TimePoint_<t>/` and `ZStep_<z>/` sub-folders; or a plate folder without its HTD, whose
//! layout is read from the file names. Calibration, objective and channel names come from the
//! plane files' MetaMorph metadata (STK tags or MetaSeries XML, decoded by the TIFF crate).
//! Layout and vocabulary: `docs/formats/imagexpress.md`; provenance:
//! `docs/provenance/imagexpress.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::model::{ChannelInfo, InstrumentInfo, ObjectiveInfo};
use openreadout_core::{Error, Fs, Result};
use openreadout_tiff::{FieldValue, metamorph, parse_metaseries};
use serde_json::{Map, Value, json};

use crate::model::{
    HcsPlate, MAX_PLATE_COLUMNS, MAX_PLATE_ROWS, PlaneFile, PlaneSlot, PlateBuilder, RawPlane,
    number,
};

/// Format id.
pub const IMAGEXPRESS_FORMAT_ID: &str = "imagexpress";

/// Largest HTD read.
const MAX_HTD_BYTES: u64 = 16 << 20;
/// Most sub-folders (`TimePoint_<t>`, `ZStep_<z>`) followed.
const MAX_SUBFOLDERS: usize = 100_000;

/// Does this text look like an HTD plate description?
pub fn looks_like_htd(head: &[u8]) -> bool {
    let h = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    h.starts_with(b"\"HTSInfoFile\"")
}

/// The HTD of a plate folder (or of its `TimePoint_1` sub-folder).
pub fn find_htd(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    for d in [dir.to_path_buf(), dir.join("TimePoint_1")] {
        let Ok(rd) = fs.read_dir(&d) else { continue };
        let mut found: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("htd"))
            })
            .collect();
        found.sort();
        if let Some(p) = found.into_iter().next() {
            return Some(p);
        }
    }
    None
}

/// A parsed HTD: key → values (quotes removed), keys in file order.
#[derive(Debug, Default, Clone)]
pub struct Htd {
    /// `(key, values)` in file order.
    pub entries: Vec<(String, Vec<String>)>,
}

impl Htd {
    /// First value of `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| v.first())
            .map(String::as_str)
    }
    /// All values of `key`.
    pub fn values(&self, key: &str) -> Option<&[String]> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_slice())
    }
    /// `key` as an unsigned number.
    pub fn uint(&self, key: &str) -> Option<u32> {
        self.get(key)?.trim().parse().ok()
    }
    /// `key` as a boolean (`TRUE`/`FALSE`).
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key)?.trim().to_ascii_uppercase().as_str() {
            "TRUE" => Some(true),
            "FALSE" => Some(false),
            _ => None,
        }
    }
}

/// Split one HTD line into quoted/unquoted comma-separated fields.
fn split_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// Parse HTD text. Errors when the first line is not the `"HTSInfoFile"` header.
pub fn parse_htd(text: &str) -> Result<Htd> {
    let mut htd = Htd::default();
    let mut first = true;
    for raw in text.lines() {
        let line = raw.trim_matches(|c: char| c == '\u{feff}' || c.is_whitespace());
        if line.is_empty() {
            continue;
        }
        let mut f = split_line(line);
        if f.is_empty() {
            continue;
        }
        let key = f.remove(0);
        if first {
            if key != "HTSInfoFile" {
                return Err(Error::corrupt(
                    IMAGEXPRESS_FORMAT_ID,
                    "not an HTD plate description (the first key is not \"HTSInfoFile\")",
                ));
            }
            first = false;
        }
        if key == "EndFile" {
            break;
        }
        if htd.entries.len() > 100_000 {
            return Err(Error::corrupt(
                IMAGEXPRESS_FORMAT_ID,
                "the HTD has more than 100,000 lines",
            ));
        }
        htd.entries.push((key, f));
    }
    if first {
        return Err(Error::corrupt(
            IMAGEXPRESS_FORMAT_ID,
            "the HTD file is empty",
        ));
    }
    Ok(htd)
}

/// A plane file name taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IxName {
    /// Text before the well (the plate name MetaXpress writes).
    pub prefix: String,
    /// Zero-based row.
    pub row: u32,
    /// Zero-based column.
    pub column: u32,
    /// Site number (1 when the name has none).
    pub site: u32,
    /// Wavelength number (1 when the name has none).
    pub wave: u32,
    /// True for `_thumb` thumbnails.
    pub thumb: bool,
}

fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Parse `<prefix>_<well>[_s<site>][_w<wave>][_thumb][<GUID>|_[<GUID>]].tif`.
pub fn parse_name(name: &str) -> Option<IxName> {
    let ext_len = if crate::model::has_extension(name, &["tif"]) {
        4
    } else if crate::model::has_extension(name, &["tiff"]) {
        5
    } else {
        return None;
    };
    let mut stem = &name[..name.len() - ext_len];
    // GUID suffix: `_[GUID]` or a bare GUID right after the last token
    if let Some(s) = stem.strip_suffix(']')
        && let Some(i) = s.rfind("_[")
        && is_guid(&s[i + 2..])
    {
        stem = &s[..i];
    } else if stem.len() > 36 && is_guid(&stem[stem.len() - 36..]) {
        stem = &stem[..stem.len() - 36];
    }
    let mut thumb = false;
    if let Some(s) = stem
        .strip_suffix("_thumb")
        .or_else(|| stem.strip_suffix("_Thumb"))
    {
        stem = s;
        thumb = true;
    }
    let take_num = |stem: &str, tag: char| -> Option<(u32, usize)> {
        let i = stem.rfind('_')?;
        let t = &stem[i + 1..];
        let mut ch = t.chars();
        let first = ch.next()?;
        if !first.eq_ignore_ascii_case(&tag) {
            return None;
        }
        let digits = &t[1..];
        if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((digits.parse().ok()?, i))
    };
    let mut wave = 1;
    if let Some((w, i)) = take_num(stem, 'w') {
        wave = w;
        stem = &stem[..i];
    }
    let mut site = 1;
    if let Some((s, i)) = take_num(stem, 's') {
        site = s;
        stem = &stem[..i];
    }
    let i = stem.rfind('_')?;
    let (row, column) = openreadout_core::plate::parse_well(&stem[i + 1..])?;
    let well = &stem[i + 1..];
    if !well.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some(IxName {
        prefix: stem[..i].to_string(),
        row,
        column,
        site,
        wave,
        thumb,
    })
}

/// Folder-name suffixes of data sets other readers own (Olympus `.oif.files`, instrument `.d`,
/// `.D` and `.raw` folders, Varian `.fid`, Zarr stores, the Sciex scan folder): never a plate
/// folder, whatever TIFF files they hold.
const FOREIGN_FOLDER_SUFFIXES: &[&str] = &[
    ".oif.files",
    ".oib.files",
    ".files",
    ".d",
    ".raw",
    ".fid",
    ".zarr",
    ".wiff.scan",
];

/// Is this file the mark of a data set another reader owns (a MetaMorph `.nd` series, an
/// Olympus `.oif`/`.oir`, Micro-Manager metadata, a CellVoyager or Harmony index, an OME
/// companion file)?
fn foreign_marker(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "metadata.txt"
        || n.ends_with("_metadata.txt")
        || n == "displaysettings.json"
        || n == "measurementdata.mlf"
        || n.starts_with("index.") && crate::model::has_extension(&n, &["xml"])
        || [
            ".nd",
            ".oif",
            ".oib",
            ".oir",
            ".companion.ome",
            ".xdce",
            ".vsi",
        ]
        .iter()
        .any(|s| n.ends_with(s))
}

/// Does the plane file carry MetaMorph metadata (MetaSeries XML in its description, or the STK
/// tags), as MetaXpress writes into every plane?
fn has_metamorph_header(fs: &Fs, path: &Path) -> bool {
    let Ok((tf, _)) = crate::planes::open_first_page(fs, path) else {
        return false;
    };
    let Some(ifd) = tf.ifds.first() else {
        return false;
    };
    if ifd.field(metamorph::STK_TAG_SETTINGS).is_some() {
        return true;
    }
    matches!(ifd.field(270).map(|f| &f.value),
        Some(FieldValue::Ascii(s)) if s.trim_start().starts_with("<MetaData>"))
}

/// Is `dir` an ImageXpress plate folder without its HTD? Positive evidence only: the folder is
/// not one another reader owns (by its name or a marker file in it), at least two plane files
/// named `<plate>_<well>[_s<site>][_w<wave>]...tif` share one plate prefix with MetaXpress well
/// names (row letters and a two-digit column, `A01`), and the first of them carries MetaMorph
/// metadata. Returns the number of plane files seen.
pub fn plate_folder_without_htd(fs: &Fs, dir: &Path) -> Option<usize> {
    let dir_name = dir.file_name()?.to_string_lossy().to_ascii_lowercase();
    if FOREIGN_FOLDER_SUFFIXES
        .iter()
        .any(|s| dir_name.ends_with(s))
    {
        return None;
    }
    let mut by_prefix: HashMap<String, Vec<String>> = HashMap::new();
    for e in fs.read_dir(dir).ok()?.flatten().take(5000) {
        let name = e.file_name().to_string_lossy().into_owned();
        if foreign_marker(&name) {
            return None;
        }
        let Some(p) = parse_name(&name) else { continue };
        if p.thumb || !metaxpress_well(&name, &p) {
            continue;
        }
        by_prefix.entry(p.prefix.clone()).or_default().push(name);
    }
    let (_, mut names) = by_prefix
        .into_iter()
        .max_by_key(|(k, v)| (v.len(), k.clone()))?;
    if names.len() < 2 {
        return None;
    }
    names.sort();
    has_metamorph_header(fs, &dir.join(&names[0])).then_some(names.len())
}

/// The well token of a parsed name has MetaXpress's spelling: one or two row letters and a
/// two-digit column (`A01`, `AF48`), not `C001` (an Olympus channel file) or `A1`.
fn metaxpress_well(name: &str, p: &IxName) -> bool {
    let token = format!("_{}", openreadout_core::plate::well_name(p.row, p.column));
    name.get(p.prefix.len()..)
        .is_some_and(|rest| rest.starts_with(&token))
}

/// `TimePoint_<n>` / `ZStep_<n>` folder number.
fn folder_number(name: &str, tag: &str) -> Option<u32> {
    let rest = name.strip_prefix(tag)?;
    rest.parse().ok()
}

/// Plane files found under the plate folder: (relative name, parsed name, t, z).
fn scan(fs: &Fs, base: &Path) -> Vec<(String, IxName, u32, u32)> {
    let mut out = Vec::new();
    let mut folders: Vec<(PathBuf, String, u32, u32)> =
        vec![(base.to_path_buf(), String::new(), 1, 1)];
    let mut seen = 0usize;
    while let Some((dir, rel, t, z)) = folders.pop() {
        let Ok(rd) = fs.read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().is_ok_and(|m| m.is_dir());
            if is_dir {
                if seen >= MAX_SUBFOLDERS {
                    continue;
                }
                let sub_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if let Some(n) = folder_number(&name, "TimePoint_") {
                    if rel.is_empty() {
                        seen += 1;
                        folders.push((e.path(), sub_rel, n, z));
                    }
                } else if let Some(n) = folder_number(&name, "ZStep_") {
                    seen += 1;
                    folders.push((e.path(), sub_rel, t, n));
                }
                continue;
            }
            if let Some(p) = parse_name(&name) {
                let full = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                out.push((full, p, t, z));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Metadata of one plane file's MetaMorph headers.
#[derive(Debug, Default, Clone)]
struct FileMeta {
    name: Option<String>,
    wavelength_nm: Option<f64>,
    pixel_size_um: [Option<f64>; 2],
    objective: Option<String>,
    na: Option<f64>,
    exposure_ms: Option<f64>,
    stage_um: [Option<f64>; 3],
    acquired: Option<String>,
    software: Option<String>,
    serial: Option<String>,
    barcode: Option<String>,
    plate_name: Option<String>,
    description: Option<String>,
    size: Option<(u32, u32, openreadout_core::PixelType)>,
}

fn description_line(desc: &str, key: &str) -> Option<String> {
    desc.split(['\r', '\n']).find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix(key)?;
        let rest = rest.trim_start_matches([':', '\t', ' ']);
        (!rest.is_empty()).then(|| rest.trim().to_string())
    })
}

/// Read the MetaMorph metadata of one plane file (headers only).
fn file_meta(fs: &Fs, path: &Path) -> Option<FileMeta> {
    let (tf, mut src) = crate::planes::open_first_page(fs, path).ok()?;
    let ifd = tf.ifds.first()?;
    let mut m = FileMeta::default();
    if let Ok(l) = openreadout_tiff::PageLayout::from_ifd(ifd, tf.header.byte_order)
        && let Ok(pt) = l.pixel_type()
    {
        m.size = Some((l.width, l.height, pt));
    }
    let desc = match ifd.field(270).map(|f| &f.value) {
        Some(FieldValue::Ascii(s)) => Some(s.clone()),
        _ => None,
    };
    if let Some(ms) = desc.as_deref().and_then(parse_metaseries) {
        m.name = ms.illumination.clone().or(ms.image_name.clone());
        m.wavelength_nm = ms.wavelength_nm.filter(|w| *w > 0.0);
        m.pixel_size_um = [ms.pixel_size_um.0, ms.pixel_size_um.1];
        m.objective = ms.objective.clone();
        m.na = ms.objective_na;
        m.exposure_ms = ms.exposure_ms;
        m.stage_um = ms.stage_um;
        m.acquired = ms.acquired_local.clone();
        m.software = match (&ms.application, &ms.application_version) {
            (Some(a), Some(v)) => Some(format!("{a} {v}")),
            (Some(a), None) => Some(a.clone()),
            _ => None,
        };
        let prop = |k: &str| {
            ms.props.values().find_map(|sec| {
                sec.get(k).and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| Some(v.to_string()))
                })
            })
        };
        m.serial = prop("Instrument Serial Number");
        let d = prop("Description");
        if let Some(d) = &d {
            m.barcode = description_line(d, "Barcode");
            m.plate_name = description_line(d, "Plate Name");
        }
        m.description = d;
        if m.software
            .as_deref()
            .is_some_and(|s| s.starts_with("MetaMorph"))
            && let Some(v) = m
                .description
                .as_deref()
                .and_then(|d| description_line(d, "Software Version"))
        {
            m.software = Some(v);
        }
    } else if ifd.field(metamorph::STK_TAG_SETTINGS).is_some()
        && let Some(stk) = metamorph::parse_stk(ifd, &mut src)
    {
        m.name = stk.name.clone();
        m.wavelength_nm = stk.wavelengths_nm.first().copied().filter(|w| *w > 0.0);
        let unit_ok = stk
            .calibration_units
            .as_deref()
            .is_some_and(|u| matches!(u.trim(), "um" | "µm" | "micron" | "microns"));
        if stk.calibrated && unit_ok {
            m.pixel_size_um = [stk.x_calibration, stk.y_calibration];
        }
        m.exposure_ms = stk.exposure_ms;
        m.stage_um = [
            stk.stage_x.first().copied(),
            stk.stage_y.first().copied(),
            stk.absolute_z.first().copied(),
        ];
        m.description = desc.clone();
        if let Some(d) = &desc {
            m.plate_name = description_line(d, "Plate Name");
            m.barcode = description_line(d, "Barcode");
            m.serial = description_line(d, "Instrument Serial Number");
        }
        m.software = match ifd.field(305).map(|f| &f.value) {
            Some(FieldValue::Ascii(s)) => Some(s.trim().to_string()),
            _ => None,
        };
    } else {
        m.description = desc;
    }
    Some(m)
}

/// Per-plane metadata of a plane file: acquisition time, stage X/Y/Z (µm), exposure (ms).
pub type PlaneMeta = (Option<String>, [Option<f64>; 3], Option<f64>);

/// Per-plane metadata of a plane file for `frames` (stage position, time, exposure).
pub fn plane_meta(fs: &Fs, path: &Path) -> Option<PlaneMeta> {
    let m = file_meta(fs, path)?;
    Some((m.acquired, m.stage_um, m.exposure_ms))
}

/// Selected (row, column) cells of a `<prefix><row>` selection block (`WellsSelection1`, ...).
fn selection(htd: &Htd, key: &str, rows: u32, cols: u32) -> Option<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    let mut any = false;
    for r in 0..rows {
        let Some(v) = htd.values(&format!("{key}{}", r + 1)) else {
            continue;
        };
        any = true;
        for (c, val) in v.iter().enumerate().take(cols as usize) {
            if val.trim().eq_ignore_ascii_case("TRUE") {
                out.push((r, c as u32));
            }
        }
    }
    any.then_some(out)
}

/// Parse an ImageXpress plate: `entry` is an `.HTD` file or a plate folder.
pub fn parse(fs: &Fs, entry: &Path) -> Result<HcsPlate> {
    let is_dir = fs.is_dir(entry);
    let htd_path = if is_dir {
        find_htd(fs, entry)
    } else {
        Some(entry.to_path_buf())
    };
    let htd = match &htd_path {
        Some(p) => {
            let m = fs.metadata(p).map_err(|e| Error::io(p, e))?;
            if m.len() > MAX_HTD_BYTES {
                return Err(Error::corrupt(
                    IMAGEXPRESS_FORMAT_ID,
                    format!("{} is {} bytes; an HTD is a few KB", p.display(), m.len()),
                ));
            }
            let b = fs.read(p).map_err(|e| Error::io(p, e))?;
            Some((parse_htd(&String::from_utf8_lossy(&b))?, b.len() as u64))
        }
        None => None,
    };
    // the folder the plane files are relative to
    let mut base = if is_dir {
        entry.to_path_buf()
    } else {
        entry.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    if base
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| folder_number(n, "TimePoint_").is_some())
        && let Some(parent) = base.parent()
        && !fs.read_dir(&base).is_ok_and(|rd| {
            rd.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with(".tif")
            })
        })
    {
        base = parent.to_path_buf();
    }
    let files = scan(fs, &base);
    let htd_stem = htd_path
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()));
    // the plate's files: named with the HTD's stem, else the most common prefix
    let prefix = match &htd_stem {
        Some(s) if files.iter().any(|f| &f.1.prefix == s) => Some(s.clone()),
        _ => {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for f in files.iter().filter(|f| !f.1.thumb) {
                *counts.entry(f.1.prefix.as_str()).or_default() += 1;
            }
            counts
                .into_iter()
                .max_by_key(|(_, n)| *n)
                .map(|(p, _)| p.to_string())
        }
    };
    let (planes, others): (Vec<_>, Vec<_>) = files
        .into_iter()
        .filter(|f| !f.1.thumb)
        .partition(|f| prefix.as_ref().is_some_and(|p| &f.1.prefix == p));
    if htd.is_none() && planes.is_empty() {
        return Err(Error::corrupt(
            IMAGEXPRESS_FORMAT_ID,
            format!(
                "{} holds neither an HTD file nor MetaXpress plane files (<plate>_<well>_s<site>_w<wave>.tif)",
                entry.display()
            ),
        ));
    }
    let source = htd_path.clone().unwrap_or_else(|| entry.to_path_buf());
    let mut plate = HcsPlate {
        format_id: IMAGEXPRESS_FORMAT_ID,
        source,
        root: base.clone(),
        index_bytes: htd.as_ref().map_or(0, |h| h.1),
        ..HcsPlate::default()
    };
    let htd = htd.map(|h| h.0);

    // geometry and selections
    let (mut rows, mut cols) = (0, 0);
    if let Some(h) = &htd {
        rows = h.uint("YWells").unwrap_or(0);
        cols = h.uint("XWells").unwrap_or(0);
        plate.format_version = h
            .get("HTSInfoFile")
            .map(|v| format!("HTSInfoFile {}", v.trim_start_matches("Version").trim()));
    }
    let max_row = planes.iter().map(|p| p.1.row + 1).max().unwrap_or(0);
    let max_col = planes.iter().map(|p| p.1.column + 1).max().unwrap_or(0);
    if rows < max_row || cols < max_col || rows == 0 || cols == 0 {
        (rows, cols) =
            openreadout_core::plate::standard_dimensions(max_row.max(rows), max_col.max(cols));
    }
    if rows > MAX_PLATE_ROWS || cols > MAX_PLATE_COLUMNS {
        return Err(Error::corrupt(
            IMAGEXPRESS_FORMAT_ID,
            format!("plate geometry {rows} x {cols} is implausible"),
        ));
    }
    plate.rows = rows;
    plate.columns = cols;

    // channels
    let waves_found: BTreeSet<u32> = planes.iter().map(|p| p.1.wave).collect();
    let (wave_list, wave_names): (Vec<u32>, Vec<Option<String>>) = match &htd {
        Some(h) if h.flag("Waves") != Some(false) && h.uint("NWavelengths").is_some() => {
            let n = h.uint("NWavelengths").unwrap_or(1).clamp(1, 1000);
            (1..=n)
                .filter(|w| h.uint(&format!("WaveCollect{w}")) != Some(0))
                .map(|w| (w, h.get(&format!("WaveName{w}")).map(str::to_string)))
                .unzip()
        }
        Some(h) if h.flag("Waves") == Some(false) => (vec![1], vec![None]),
        _ => {
            let v: Vec<u32> = if waves_found.is_empty() {
                vec![1]
            } else {
                waves_found.iter().copied().collect()
            };
            let n = v.len();
            (v, vec![None; n])
        }
    };
    let wave_index: HashMap<u32, usize> =
        wave_list.iter().enumerate().map(|(i, &w)| (w, i)).collect();
    // one plane file per wavelength for names, wavelengths and calibration
    let mut metas: Vec<Option<FileMeta>> = vec![None; wave_list.len()];
    for (i, &w) in wave_list.iter().enumerate() {
        if let Some(f) = planes.iter().find(|p| p.1.wave == w) {
            metas[i] = file_meta(fs, &plate.path_of(&f.0));
        }
    }
    let first_meta = metas.iter().flatten().next().cloned().unwrap_or_default();
    for (i, &w) in wave_list.iter().enumerate() {
        let m = metas[i].clone().unwrap_or_default();
        plate.channels.push(ChannelInfo {
            index: i as u32,
            name: wave_names[i]
                .clone()
                .or(m.name.clone())
                .or_else(|| Some(format!("w{w}"))),
            fluorophore: None,
            excitation_nm: None,
            emission_nm: m.wavelength_nm,
            emission_range_nm: None,
            color: None,
            acquisition_mode: None,
            exposure_ms: m.exposure_ms.filter(|e| *e > 0.0),
            ..ChannelInfo::default()
        });
        let mut ex = BTreeMap::new();
        ex.insert("wave".into(), json!(w));
        if let Some(n) = &m.name
            && Some(n) != wave_names[i].as_ref()
        {
            ex.insert("illumination_setting".into(), json!(n));
        }
        plate.channel_extra.push(ex);
    }
    if let Some((w, h, pt)) = first_meta.size {
        plate.size_x = w;
        plate.size_y = h;
        plate.pixel_type = Some(pt);
        plate.pixel_source = Some("plane file header".into());
    }
    plate.pixel_size_um = first_meta.pixel_size_um;
    if first_meta.objective.is_some() || first_meta.na.is_some() {
        let mag = first_meta.objective.as_deref().and_then(|o| {
            let d: String = o
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            number(&d).filter(|_| o.to_ascii_lowercase().contains('x'))
        });
        plate.objective = Some(ObjectiveInfo {
            model: first_meta.objective.clone(),
            nominal_magnification: mag,
            lens_na: first_meta.na.filter(|n| *n > 0.0),
            immersion: None,
        });
    }
    plate.instrument = InstrumentInfo {
        manufacturer: Some("Molecular Devices".into()),
        model: None,
        software: first_meta.software.clone(),
        software_version: None,
        detector: None,
    };
    plate.id = first_meta
        .barcode
        .clone()
        .or_else(|| htd_stem.clone())
        .or_else(|| prefix.clone());
    let desc = htd
        .as_ref()
        .and_then(|h| h.get("Description"))
        .map(str::to_string);
    plate.name = first_meta
        .plate_name
        .clone()
        .or_else(|| desc.clone())
        .filter(|n| Some(n) != plate.id.as_ref());
    plate.description = desc.clone();
    plate.started_at = None;

    // expected planes
    let sites_on = htd.as_ref().and_then(|h| h.flag("Sites"));
    let mut sites: Vec<u32> = Vec::new();
    if let Some(h) = &htd {
        if sites_on == Some(true) {
            let xs = h.uint("XSites").unwrap_or(1).clamp(1, 1000);
            let ys = h.uint("YSites").unwrap_or(1).clamp(1, 1000);
            let n = selection(h, "SiteSelection", ys, xs).map_or(xs * ys, |v| v.len() as u32);
            sites = (1..=n.max(1)).collect();
        } else {
            sites = vec![1];
        }
    }
    let times: Vec<u32> = htd
        .as_ref()
        .and_then(|h| h.uint("TimePoints"))
        .map_or_else(|| vec![1], |n| (1..=n.clamp(1, 100_000)).collect());
    let zsteps: Vec<u32> = match &htd {
        Some(h) if h.flag("ZSeries") == Some(true) => {
            (1..=h.uint("ZSteps").unwrap_or(1).clamp(1, 100_000)).collect()
        }
        _ => vec![1],
    };
    let wells: Vec<(u32, u32)> = match &htd {
        Some(h) => selection(h, "WellsSelection", rows, cols).unwrap_or_default(),
        None => Vec::new(),
    };
    plate.declared_wells = wells.clone();
    let mut b = PlateBuilder::new(IMAGEXPRESS_FORMAT_ID);
    let mut found: HashMap<(u32, u32, u32, u32, u32, u32), String> = HashMap::new();
    for (name, p, t, z) in &planes {
        found
            .entry((p.row, p.column, p.site, p.wave, *z, *t))
            .or_insert_with(|| name.clone());
    }
    let mut placed: std::collections::HashSet<(u32, u32, u32, u32, u32, u32)> =
        std::collections::HashSet::new();
    let push = |b: &mut PlateBuilder,
                key: (u32, u32, u32, u32, u32, u32),
                slot: PlaneSlot|
     -> Result<()> {
        let (row, column, site, wave, z, t) = key;
        let Some(&channel) = wave_index.get(&wave) else {
            return Ok(());
        };
        b.push(RawPlane {
            row,
            column,
            field: site,
            channel,
            z,
            t,
            slot,
            field_position_um: [None, None],
        })
    };
    if htd.is_some() {
        for &(r, c) in &wells {
            for &s in &sites {
                for &w in &wave_list {
                    for &z in &zsteps {
                        for &t in &times {
                            let key = (r, c, s, w, z, t);
                            let slot = match found.get(&key) {
                                Some(n) => {
                                    placed.insert(key);
                                    PlaneSlot::File(PlaneFile {
                                        name: n.clone(),
                                        ..PlaneFile::default()
                                    })
                                }
                                None => PlaneSlot::Missing { name: None },
                            };
                            push(&mut b, key, slot)?;
                        }
                    }
                }
            }
        }
    }
    let mut outside = 0u64;
    for (key, n) in &found {
        if placed.contains(key) {
            continue;
        }
        if htd.is_some() {
            outside += 1;
        }
        push(
            &mut b,
            *key,
            PlaneSlot::File(PlaneFile {
                name: n.clone(),
                ..PlaneFile::default()
            }),
        )?;
    }
    if b.is_empty() {
        return Err(Error::corrupt(
            IMAGEXPRESS_FORMAT_ID,
            "the HTD selects no well and no plane file was found".to_string(),
        ));
    }
    if outside > 0 {
        plate.notes.push(format!(
            "{outside} plane files lie outside the wells, sites, wavelengths or time points the HTD selects; they are included"
        ));
    }
    b.finish(&mut plate)?;
    plate.unindexed_files = others.iter().map(|o| o.0.clone()).collect();
    if htd.is_none() {
        plate.notes.push(
            "no HTD plate description: wells, sites and wavelengths come from the file names, so missing planes cannot be detected".into(),
        );
    }
    let mut ex = BTreeMap::new();
    if let Some(h) = &htd {
        for (k, key) in [
            ("unique_plate_identifier", "UniquePlateIdentifier"),
            ("plate_type_code", "PlateType"),
        ] {
            if let Some(v) = h.get(key) {
                ex.insert(k.into(), json!(v));
            }
        }
        if let Some(s) = h.flag("Sites") {
            ex.insert("sites".into(), json!(s));
        }
        if let (Some(x), Some(y)) = (h.uint("XSites"), h.uint("YSites")) {
            ex.insert("site_grid".into(), json!([x, y]));
        }
        if let Some(z) = h.flag("ZProjection") {
            ex.insert("z_projection".into(), json!(z));
        }
    }
    if let Some(s) = &first_meta.serial {
        ex.insert("instrument_serial".into(), json!(s));
    }
    if let Some(p) = &prefix {
        ex.insert("file_prefix".into(), json!(p));
    }
    plate.plate_extra = ex;
    let mut vendor = Map::new();
    if let Some(h) = &htd {
        let mut m = Map::new();
        for (k, v) in &h.entries {
            m.insert(k.clone(), if v.len() == 1 { json!(v[0]) } else { json!(v) });
        }
        vendor.insert("HTD".into(), Value::Object(m));
    }
    if let Some(d) = &first_meta.description {
        vendor.insert("first plane file description".into(), json!(d));
    }
    plate.vendor = Value::Object(vendor);
    Ok(plate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_taken_apart() {
        let p = parse_name("HTS_A01_s1_w14996E914-A4B6-49AF-8081-5B58A8653201.tif").unwrap();
        assert_eq!(
            (p.prefix.as_str(), p.row, p.column, p.site, p.wave, p.thumb),
            ("HTS", 0, 0, 1, 1, false)
        );
        let p = parse_name("HTS_A01_s2_w5_thumb0D3C156F-BBC5-4C14-87F8-5EE80C823C51.tif").unwrap();
        assert!(p.thumb);
        assert_eq!((p.site, p.wave), (2, 5));
        let p = parse_name("BSF018292-1A_P24_w2.TIF").unwrap();
        assert_eq!(
            (p.prefix.as_str(), p.row, p.column, p.site, p.wave),
            ("BSF018292-1A", 15, 23, 1, 2)
        );
        let p = parse_name("2011-04-19-plate-1_A01_s1_[A192FCC1-DC1A-4523-97BB-07688327EAC3].tif")
            .unwrap();
        assert_eq!(
            (p.prefix.as_str(), p.site, p.wave),
            ("2011-04-19-plate-1", 1, 1)
        );
        let p = parse_name("plate 11001_H12_s16_w2.TIF").unwrap();
        assert_eq!((p.row, p.column, p.site), (7, 11, 16));
        assert_eq!(parse_name("notes.txt"), None);
        assert_eq!(parse_name("r01c01f01p01-ch1sk1fk1fl1.tiff"), None);
    }

    #[test]
    fn htd_is_parsed() {
        let t = "\"HTSInfoFile\", Version 1.0\r\n\"Description\", \"a, b\"\r\n\"XWells\", 12\r\n\"YWells\", 8\r\n\"WellsSelection1\", TRUE, FALSE\r\n\"Sites\", TRUE\r\n\"XSites\", 2\r\n\"YSites\", 2\r\n\"SiteSelection1\", TRUE, FALSE\r\n\"SiteSelection2\", TRUE, TRUE\r\n\"EndFile\"\r\n\"Ignored\", 1";
        let h = parse_htd(t).unwrap();
        assert_eq!(h.get("Description"), Some("a, b"));
        assert_eq!(h.uint("XWells"), Some(12));
        assert_eq!(h.flag("Sites"), Some(true));
        assert_eq!(selection(&h, "SiteSelection", 2, 2).unwrap().len(), 3);
        assert_eq!(
            selection(&h, "WellsSelection", 8, 12).unwrap(),
            vec![(0, 0)]
        );
        assert!(h.get("Ignored").is_none());
        assert!(parse_htd("\"Other\", 1").is_err());
        assert!(parse_htd("").is_err());
    }
}
