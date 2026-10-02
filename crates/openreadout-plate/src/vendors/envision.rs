//! PerkinElmer / Revvity EnVision Workstation CSV exports and Revvity Kaleido CSV exports
//! (EnVision Nexus, EnSight, VICTOR Nivo).
//!
//! EnVision: per plate, `Plate information` (a header line and a value line: plate number,
//! repeat, barcode as `="…"`, temperatures, humidity, the read's `Label` and `Measinfo`,
//! measurement date), `Background information` (label, `MeasInfo`), then
//! `Results for <label>(<n>) - channel <k> (<unit>)` or `Calculated results: …` followed by a
//! plate matrix, or a bare matrix (no title, no row letters, no column numbers). The export's
//! `Auto export parameters` say whether the plate information comes before its plate's data
//! (`Place plate information at … Beginning of plate`) or after it (`End of plate`). After the
//! plates: `Basic assay information`, `Protocol information`, `Labels:` (`<label>,,,,<id>` or
//! `Label name,,,,<label>`), `Filters:` (`Description … CWL=450nm BW=10nm`), `Instrument:`.
//! Kaleido: `Results for <technology>` + `Barcode: …, PlateRepeat: n, WellRepeat: n` + matrix,
//! then `Measurement Basic Information` (`Key:,,value`), `Plate Type`, `Measurements:`.

use std::collections::BTreeMap;

use openreadout_core::model::Finding;
use serde_json::json;

use crate::grid::{Grid, dims_for_wells, find_grids};
use crate::model::{
    Block, Channel, Export, Kind, Mode, ModeBasis, ReadType, first_number, nonempty,
};
use crate::sheet::{Book, Sheet};
use crate::text::unformula;

/// Lines searched for the EnVision markers.
const SNIFF_LINES: usize = 4000;

pub(crate) fn sniff_envision(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .take(SNIFF_LINES)
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .filter(|l| !l.is_empty())
        .collect();
    let plate_info = lines.windows(2).any(|w| {
        w[0].starts_with("Plate information")
            && [
                "Plate,Repeat,Barcode",
                "Plate;Repeat;Barcode",
                "Plate\tRepeat\tBarcode",
            ]
            .iter()
            .any(|p| w[1].starts_with(p))
    });
    // The plate information starts the file, or the file names its writer or carries the
    // EnVision trailer sections (plate information after the data, or the assay information
    // first).
    plate_info
        && (lines
            .first()
            .is_some_and(|l| l.starts_with("Plate information"))
            || text.contains("Exported with EnVision")
            || (text.contains("Basic assay information") && text.contains("Protocol information")))
}

pub(crate) fn sniff_kaleido(text: &str) -> bool {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim_start_matches('\u{feff}')
        .trim();
    (first.starts_with("Results for")
        || first.starts_with("EnSight Results from")
        || first.starts_with("Kaleido Results from"))
        && (text.contains("Kaleido")
            || text.contains("Measurement Basic Information")
            || text.contains("Measurement Information"))
}

/// `Name,,,,value` lines (EnVision) or `Name:,,value` lines (Kaleido): first and last cell.
fn kv(sheet: &Sheet, r: usize) -> Option<(String, String)> {
    let cells = sheet.row_texts(r);
    match cells.as_slice() {
        [k, .., v] => Some((
            k.trim_end_matches(':').trim().to_string(),
            unformula(v).to_string(),
        )),
        _ => None,
    }
}

/// A bare matrix (no row letters, no column numbers): consecutive lines of numbers only,
/// all the same width, directly after a results title.
fn bare_matrix(sheet: &Sheet, start: usize) -> Option<(usize, usize, usize)> {
    let width = sheet.row_len(start);
    if width < 2 {
        return None;
    }
    let is_num_row = |r: usize| {
        sheet.row_len(r) == width
            && (0..width)
                .all(|c| sheet.cell(r, c).number().is_some() || sheet.cell(r, c).is_blank())
            && sheet.cell(r, 0).number().is_some()
    };
    let mut r = start;
    while r < sheet.rows.len() && is_num_row(r) {
        r += 1;
    }
    (r > start).then_some((start, r, width))
}

/// A bare matrix whose lines all start with an empty field and have the same number of fields,
/// every non-blank cell a number, and whose size is a standard plate (rows × columns): the
/// lines of wells that were not read stay in it as empty fields. Returns (first line, end,
/// first value column, columns).
fn padded_bare_matrix(sheet: &Sheet, start: usize) -> Option<(usize, usize, usize, usize)> {
    let fields = sheet.rows.get(start)?.len();
    if fields < 3 {
        return None;
    }
    let fits = |r: usize| {
        sheet.rows.get(r).is_some_and(|row| {
            row.len() == fields
                && row[0].is_blank()
                && row.iter().all(|c| c.is_blank() || c.number().is_some())
        })
    };
    let mut end = start;
    while fits(end) {
        end += 1;
    }
    let rows = end - start;
    // The field count is the plate's width: wells not read are empty fields at either edge.
    let cols = fields - 1;
    let any_value = (start..end).any(|r| !sheet.row_is_blank(r));
    let standard = u32::try_from(rows * cols)
        .ok()
        .and_then(dims_for_wells)
        .is_some_and(|(pr, pc)| pr as usize == rows && pc as usize == cols);
    (any_value && standard).then_some((start, end, 1, cols))
}

struct Filter {
    cwl: Option<f64>,
    bw: Option<f64>,
}

fn filters(sheet: &Sheet, start: usize) -> BTreeMap<String, Filter> {
    let mut out = BTreeMap::new();
    let mut r = start;
    while r + 1 < sheet.rows.len() {
        if sheet.text(r, 0).starts_with("Instrument") {
            break;
        }
        if sheet.text(r + 1, 0) == "Filter type" {
            let name = sheet.text(r, 0);
            let mut f = Filter {
                cwl: None,
                bw: None,
            };
            for rr in r + 1..(r + 8).min(sheet.rows.len()) {
                if sheet.text(rr, 0) == "Description" {
                    let d = sheet.row_texts(rr).last().cloned().unwrap_or_default();
                    f.cwl = d.split("CWL=").nth(1).and_then(first_number);
                    f.bw = d.split("BW=").nth(1).and_then(first_number);
                }
            }
            out.insert(name, f);
        }
        r += 1;
    }
    out
}

/// The detection mode a label names, by keywords (`A450`, `ABS`, `US LUM 384`, `AC HTRF Laser
/// [Eu]`, `FP`, `Alpha`). `None` when no keyword is present: labels are user-editable names.
pub(crate) fn label_mode(label: &str) -> Option<Mode> {
    let tokens: Vec<String> = label
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let any = |f: &dyn Fn(&str) -> bool| tokens.iter().any(|t| f(t));
    if any(&|t| {
        t.starts_with("absorb")
            || t == "abs"
            || t == "od"
            || t.strip_prefix("od").is_some_and(digits)
            || (t.len() >= 3 && t.strip_prefix('a').is_some_and(digits))
    }) {
        Some(Mode::Absorbance)
    } else if any(&|t| t.starts_with("alpha")) {
        Some(Mode::Alpha)
    } else if any(&|t| t.starts_with("lum")) {
        Some(Mode::Luminescence)
    } else if any(&|t| {
        t.starts_with("fluor") || matches!(t, "fi" | "fp" | "htrf" | "trf" | "fret" | "lance")
    }) {
        Some(Mode::Fluorescence)
    } else {
        None
    }
}

/// The detection mode the measurement information of a read names (`MeasInfo`
/// `De=USLum Ex=N/A Em=N/A Wdw=N/A (12)`): a luminescence or Alpha detector, or the optics
/// of a photometric read (excitation light, no emission path) or a fluorescence read (both).
pub(crate) fn detector_mode(measinfo: &str) -> Option<Mode> {
    let field = |key: &str| {
        measinfo
            .split_whitespace()
            .find_map(|w| w.strip_prefix(key))
            .map(str::to_ascii_lowercase)
    };
    let de = field("De=")?;
    let ex = field("Ex=").unwrap_or_default();
    let em = field("Em=").unwrap_or_default();
    let optic = |s: &str| matches!(s, "top" | "btm" | "bottom");
    if de.contains("lum") {
        Some(Mode::Luminescence)
    } else if de.contains("alpha") {
        Some(Mode::Alpha)
    } else if optic(&ex) && em == "n/a" {
        Some(Mode::Absorbance)
    } else if optic(&ex) && optic(&em) {
        Some(Mode::Fluorescence)
    } else {
        None
    }
}

/// `US LUM 1536 (cps)(1)` → `US LUM 1536 (cps)`: a label as the plate information and results
/// titles write it (with the label's number) → as the labels and background sections do.
fn bare_label(s: &str) -> &str {
    let t = s.trim();
    if let Some(open) = t.rfind('(')
        && t.ends_with(')')
        && t[open + 1..t.len() - 1].chars().all(|c| c.is_ascii_digit())
        && open + 2 < t.len()
    {
        return t[..open].trim_end();
    }
    t
}

/// One `Plate information` block.
struct PlateInfo {
    /// Line of the `Plate information` title.
    line: usize,
    key: String,
    number: String,
    barcode: Option<String>,
    fields: Vec<(String, String)>,
}

impl PlateInfo {
    fn get(&self, k: &str) -> Option<String> {
        self.fields
            .iter()
            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
            .and_then(|v| nonempty(&v))
    }
}

fn plate_info(sheet: &Sheet, r: usize) -> PlateInfo {
    let names = sheet.rows.get(r + 1).cloned().unwrap_or_default();
    let vals = sheet.rows.get(r + 2).cloned().unwrap_or_default();
    let fields: Vec<(String, String)> = names
        .iter()
        .zip(vals.iter())
        .map(|(k, v)| (k.trimmed(), unformula(&v.trimmed()).to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    let mut info = PlateInfo {
        line: r,
        key: String::new(),
        number: String::new(),
        barcode: None,
        fields,
    };
    info.number = info.get("Plate").unwrap_or_else(|| "1".into());
    info.barcode = info.get("Barcode");
    info.key = match info.get("Repeat") {
        Some(rp) if rp != "1" => format!("{} repeat {rp}", info.number),
        _ => info.number.clone(),
    };
    info
}

/// How a read's mode is resolved: the detector its measurement information names, else the
/// keywords of its label, else unknown. A disagreement between the two is reported.
struct ModeResolver {
    /// Detector modes by bare label.
    detectors: BTreeMap<String, Mode>,
    /// The label of the file's `Labels:` section, when there is one.
    labels_section: Option<String>,
}

impl ModeResolver {
    fn resolve(&self, label: &str, findings: &mut Vec<Finding>) -> (Mode, ModeBasis) {
        let bare = bare_label(label);
        let from_detector = self.detectors.get(bare).copied().or_else(|| {
            // one read in the file: its detector, whatever the label is called
            (self.detectors.len() == 1)
                .then(|| self.detectors.values().next().copied())
                .flatten()
        });
        let from_label = label_mode(bare).or_else(|| {
            if bare.is_empty() {
                self.labels_section.as_deref().and_then(label_mode)
            } else {
                None
            }
        });
        match (from_detector, from_label) {
            (Some(d), Some(l)) if d != l => {
                if !findings.iter().any(|f| f.code == "read_mode_conflict") {
                    findings.push(Finding::warning(
                        "read_mode_conflict",
                        format!(
                            "read {label:?}: its detector says {}, its label says {}; the detector's mode is used",
                            d.name(),
                            l.name()
                        ),
                    ));
                }
                (d, ModeBasis::Detector)
            }
            (Some(d), _) => (d, ModeBasis::Detector),
            (None, Some(l)) => (l, ModeBasis::Label),
            (None, None) => (Mode::Unknown, ModeBasis::Undetermined),
        }
    }
}

pub(crate) fn parse_envision(book: &Book) -> Export {
    let sheet = &book.sheets[0];
    let mut ex = Export::new(Kind::EnVision, book.container.clone());
    let n = sheet.rows.len();
    // Trailer sections first: labels, filters, instrument, assay information.
    let mut labels: Vec<(String, String)> = Vec::new();
    let mut label_name: Option<String> = None;
    let mut filter_map = BTreeMap::new();
    let mut section = String::new();
    let mut protocol = Vec::new();
    let mut auto_export: Vec<(String, String)> = Vec::new();
    // `Calculations:` → `Formula index` (`Calc 1`) → `Formula` text
    let mut formulas: BTreeMap<String, String> = BTreeMap::new();
    let mut formula_index: Option<String> = None;
    // Lines inside the assay, protocol and settings sections (before or after the plates):
    // no plate data there.
    let mut in_trailer = vec![false; n];
    let mut trailer = false;
    for (r, slot) in in_trailer.iter_mut().enumerate() {
        let t0 = sheet.text(r, 0);
        if t0.starts_with("Results for") || t0.starts_with("Calculated results") {
            trailer = false;
        }
        *slot = trailer;
        let is_title = sheet.row_len(r) == 1 && t0.ends_with(':')
            || matches!(
                t0.as_str(),
                "Basic assay information"
                    | "Protocol information"
                    | "Plate information"
                    | "Background information"
            );
        if is_title {
            section = t0.trim_end_matches(':').trim().to_string();
            if section == "Filters" {
                filter_map = filters(sheet, r + 1);
            }
            trailer = !matches!(
                section.as_str(),
                "Plate information" | "Background information"
            );
            *slot = trailer;
            continue;
        }
        match section.as_str() {
            "Basic assay information" => {
                if let Some((k, v)) = kv(sheet, r) {
                    ex.put(k, v);
                }
            }
            "Labels" => {
                if let Some((k, v)) = kv(sheet, r) {
                    // `<label>,,,,<id>` (the label first), or `Label name,,,,<label>`
                    if k == "Label name" {
                        label_name.get_or_insert_with(|| v.clone());
                    } else if label_name.is_none() {
                        label_name = Some(k.clone());
                    }
                    labels.push((k, v));
                }
            }
            "Instrument" => {
                if let Some((k, v)) = kv(sheet, r) {
                    ex.put(format!("Instrument {k}"), v);
                } else if t0.starts_with("Exported with") {
                    ex.put("Exported with", t0.clone());
                }
            }
            "Protocol" | "Plate type" | "Protocol information" => {
                if let Some((k, v)) = kv(sheet, r) {
                    protocol.push((k, v));
                }
            }
            "Auto export parameters" => {
                if let Some((k, v)) = kv(sheet, r) {
                    auto_export.push((k, v));
                }
            }
            "Calculations" => match kv(sheet, r) {
                Some((k, v)) if k == "Formula index" => formula_index = Some(v),
                Some((k, v)) if k == "Formula" && !v.is_empty() => {
                    if let Some(i) = formula_index.take() {
                        formulas.entry(i).or_insert(v);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        if t0.starts_with("Exported with") {
            ex.software_version = t0.rsplit(' ').next().map(str::to_string);
        }
    }
    let label_get = |k: &str| {
        labels
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.clone())
    };
    let auto_get = |k: &str| {
        auto_export
            .iter()
            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
    };
    ex.serial = ex
        .get("Instrument Serial number")
        .or_else(|| ex.get("Serial#"))
        .map(str::to_string);
    ex.model = Some("EnVision".into());
    if let Some(nick) = ex.get("Instrument Nickname") {
        ex.put("Nickname", nick.to_string());
    }
    ex.protocol = ex.get("Protocol Name").map(str::to_string);
    let wells_declared = protocol
        .iter()
        .find(|(k, _)| k == "Number of the wells in the plate")
        .and_then(|(_, v)| v.parse::<u32>().ok());
    let plate_type = protocol
        .iter()
        .find(|(k, _)| k == "Name of the plate type")
        .map(|(_, v)| v.clone());
    ex.sections.insert(
        "protocol".into(),
        json!(
            protocol
                .iter()
                .map(|(k, v)| json!([k, v]))
                .collect::<Vec<_>>()
        ),
    );
    if !labels.is_empty() {
        ex.sections.insert(
            "labels".into(),
            json!(
                labels
                    .iter()
                    .map(|(k, v)| json!([k, v]))
                    .collect::<Vec<_>>()
            ),
        );
    }
    if !auto_export.is_empty() {
        ex.sections.insert(
            "auto_export".into(),
            json!(
                auto_export
                    .iter()
                    .map(|(k, v)| json!([k, v]))
                    .collect::<Vec<_>>()
            ),
        );
    }
    let exc = label_get("Exc. filter").and_then(|f| filter_map.get(&f).and_then(|x| x.cwl));
    let em1 = label_get("Ems. filter").and_then(|f| filter_map.get(&f).and_then(|x| x.cwl));
    let em2 = label_get("2nd ems. filter").and_then(|f| filter_map.get(&f).and_then(|x| x.cwl));
    let exc_bw = label_get("Exc. filter").and_then(|f| filter_map.get(&f).and_then(|x| x.bw));
    let flashes = label_get("Number of flashes").and_then(|v| crate::sheet::parse_number(&v));

    // Plate information blocks, and which one owns the data at a line.
    let infos: Vec<PlateInfo> = (0..n)
        .filter(|&r| sheet.text(r, 0) == "Plate information")
        .map(|r| plate_info(sheet, r))
        .collect();
    let info_at_end = auto_get("Place plate information at")
        .is_some_and(|v| v.to_ascii_lowercase().contains("end"));
    if info_at_end {
        ex.notes.push(
            "the export places each plate's information after its data (`Place plate information at: End of plate`)".into(),
        );
    }
    let owner = |r: usize| -> Option<&PlateInfo> {
        let before = infos.iter().rev().find(|i| i.line < r);
        let after = infos.iter().find(|i| i.line > r);
        if info_at_end {
            after.or(before)
        } else {
            before.or(after)
        }
    };
    // Detector modes from the plate and background information (`Label` + `Measinfo`).
    let mut detectors: BTreeMap<String, Mode> = BTreeMap::new();
    for r in 0..n {
        let t0 = sheet.text(r, 0);
        if t0 != "Plate information" && t0 != "Background information" {
            continue;
        }
        let names = sheet.rows.get(r + 1).cloned().unwrap_or_default();
        let li = names.iter().position(|c| c.trimmed() == "Label");
        let mi = names
            .iter()
            .position(|c| c.trimmed().eq_ignore_ascii_case("MeasInfo"));
        let (Some(li), Some(mi)) = (li, mi) else {
            continue;
        };
        let mut rr = r + 2;
        while rr < n && !sheet.row_is_blank(rr) && sheet.row_len(rr) > 1 {
            let label = sheet.text(rr, li);
            if let Some(m) = detector_mode(&sheet.text(rr, mi))
                && !label.is_empty()
            {
                detectors.insert(bare_label(&label).to_string(), m);
            }
            rr += 1;
        }
    }
    let resolver = ModeResolver {
        detectors,
        labels_section: label_name.clone(),
    };

    let grids = find_grids(sheet);
    let mut blocks: BTreeMap<String, Block> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    // The block of the plate that owns line `r`, created with its plate information.
    let mut block_at = |r: usize, blocks: &mut BTreeMap<String, Block>| -> String {
        let info = owner(r);
        let key = info.map_or_else(|| "1".to_string(), |i| i.key.clone());
        blocks.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let mut b = Block::new(
                format!("Plate {key}"),
                info.and_then(|i| i.barcode.clone()).unwrap_or_else(|| {
                    format!("Plate {}", info.map_or("1", |i| i.number.as_str()))
                }),
            );
            b.barcode = info.and_then(|i| i.barcode.clone());
            b.read_type = Some(ReadType::Endpoint);
            b.line = Some(r + 1);
            b.declared_wells = wells_declared;
            b.plate_type.clone_from(&plate_type);
            b
        });
        key
    };
    // Every plate information block fills its plate's metadata (the first value wins).
    for info in &infos {
        let key = block_at(
            if info_at_end {
                info.line.saturating_sub(1)
            } else {
                info.line + 1
            },
            &mut blocks,
        );
        let Some(entry) = blocks.get_mut(&key) else {
            continue;
        };
        if let Some(md) = info.get("Measurement date")
            && entry.started_at.is_none()
            && let Some((iso, _)) = crate::datetime::combine(&md, None)
        {
            entry.started_at = Some(iso);
        }
        for (k, key) in [
            (
                "Chamber temperature at start",
                "chamber_temperature_start_c",
            ),
            ("Chamber temperature at end", "chamber_temperature_end_c"),
            ("Inside temperature at start", "chamber_temperature_start_c"),
            ("Inside temperature at end", "chamber_temperature_end_c"),
            ("Measured height", "measured_height_mm"),
            ("Humidity at start", "humidity_start_percent"),
            (
                "Ambient temperature at start",
                "ambient_temperature_start_c",
            ),
        ] {
            if let Some(v) = info.get(k).and_then(|v| crate::sheet::parse_number(&v)) {
                entry.extra.entry(key.into()).or_insert(json!(v));
                if key == "chamber_temperature_start_c" && entry.temperature_c.is_none() {
                    entry.temperature_c = Some(v);
                }
            }
        }
    }
    // A measured read's channel: mode from its detector or label, optics from the labels.
    let measured = |label: &str, title: &str, findings: &mut Vec<Finding>| -> Channel {
        let (mode, basis) = resolver.resolve(label, findings);
        let mut ch = Channel::derived(title, mode, basis);
        let channel_no = title
            .split("channel")
            .nth(1)
            .and_then(first_number)
            .map(|v| v as u32);
        ch.unit = title
            .rsplit_once(" (")
            .filter(|(_, u)| u.ends_with(')'))
            .map(|(_, u)| u.trim_end_matches(')').to_string());
        match mode {
            Mode::Absorbance => ch.wavelength_nm = exc.or_else(|| first_number(label)),
            Mode::Luminescence => ch.emission_nm = if channel_no == Some(2) { em2 } else { em1 },
            Mode::Unknown => {}
            _ => {
                ch.excitation_nm = exc;
                ch.emission_nm = if channel_no == Some(2) { em2 } else { em1 };
            }
        }
        if let Some(bw) = exc_bw.filter(|_| mode == Mode::Absorbance) {
            ch.settings.insert("bandwidth_nm".into(), json!(bw));
        }
        if let Some(f) = flashes {
            ch.settings.insert("flashes".into(), json!(f));
        }
        ch
    };

    let mut r = 0;
    while r < n {
        let t0 = sheet.text(r, 0);
        if t0 == "Plate information" {
            r += 3;
            continue;
        }
        // Some exports put the matrix right after the background rows, with no results title.
        if t0 == "Background information" {
            let mut rr = r + 2;
            let mut bg_label = None;
            while rr < n
                && sheet.cell(rr, 0).number().is_some()
                && sheet.cell(rr, 1).number().is_none()
                && !sheet.row_is_blank(rr)
            {
                bg_label.get_or_insert_with(|| sheet.text(rr, 1));
                rr += 1;
            }
            if let Some((a, z, w)) = bare_matrix(sheet, rr) {
                let key = block_at(rr, &mut blocks);
                let label = bg_label.unwrap_or_default();
                let ch = measured(&label, &label, &mut findings);
                let block = blocks.get_mut(&key).expect("block_at inserts the key");
                let idx = block.channel(ch);
                for (i, row) in (a..z).enumerate() {
                    for c in 0..w {
                        let cell = sheet.cell(row, c).clone();
                        block.push_cell(i as u32, c as u32, idx, None, None, &cell);
                    }
                }
                block.extra.insert("unlabelled_matrix".into(), json!(true));
                r = z;
                continue;
            }
            r = rr;
            continue;
        }
        let results = t0.starts_with("Results for");
        let calc = t0.starts_with("Calculated results");
        if results || calc {
            let key = block_at(r, &mut blocks);
            let title = t0.clone();
            let label = title
                .trim_start_matches("Results for")
                .trim_start_matches("Calculated results:")
                .trim()
                .to_string();
            let ch = if calc {
                let mut ch = Channel::new(label.clone(), Mode::Unknown);
                ch.calculated = true;
                ch
            } else {
                let read_label = label.split(" - channel").next().unwrap_or(&label);
                measured(read_label, &label, &mut findings)
            };
            let block = blocks.get_mut(&key).expect("block_at inserts the key");
            let idx = block.channel(ch);
            // A lettered matrix right after the title, or a bare one.
            if let Some(g) = grids
                .iter()
                .find(|g| g.header_row > r && g.header_row <= r + 2)
            {
                push(sheet, g, block, idx);
                r = g.end_row;
            } else if let Some((a, z, w)) = bare_matrix(sheet, r + 1) {
                for (i, rr) in (a..z).enumerate() {
                    for c in 0..w {
                        let cell = sheet.cell(rr, c).clone();
                        block.push_cell(i as u32, c as u32, idx, None, None, &cell);
                    }
                }
                block.extra.insert("unlabelled_matrix".into(), json!(true));
                r = z;
            } else {
                r += 1;
            }
            continue;
        }
        // A bare matrix with no title at all: the read is the one its plate information names.
        // Only a standard plate's size is accepted, so that no other block of numbers is taken
        // for one.
        let untitled = (!in_trailer[r])
            .then(|| bare_matrix(sheet, r))
            .flatten()
            .filter(|(a, z, w)| {
                let rows = u32::try_from(z - a).unwrap_or(0);
                let cols = u32::try_from(*w).unwrap_or(0);
                rows.checked_mul(cols).and_then(dims_for_wells) == Some((rows, cols))
            })
            .map(|(a, z, w)| (a, z, 0, w))
            .or_else(|| {
                (!in_trailer[r])
                    .then(|| padded_bare_matrix(sheet, r))
                    .flatten()
            });
        if let Some((a, z, c0, w)) = untitled {
            let key = block_at(r, &mut blocks);
            let label = owner(r).and_then(|i| i.get("Label")).unwrap_or_default();
            let mut ch = measured(&label, &label, &mut findings);
            if ch.label.is_empty() {
                ch.label = "Plate matrix".into();
            }
            let block = blocks.get_mut(&key).expect("block_at inserts the key");
            let idx = block.channel(ch);
            for (i, rr) in (a..z).enumerate() {
                for c in 0..w {
                    let cell = sheet.cell(rr, c0 + c).clone();
                    block.push_cell(i as u32, c as u32, idx, None, None, &cell);
                }
            }
            block.extra.insert("unlabelled_matrix".into(), json!(true));
            r = z;
            continue;
        }
        r += 1;
    }
    for k in order {
        if let Some(mut b) = blocks.remove(&k) {
            if b.declared_wells.is_none() {
                b.declared_wells = wells_declared;
            }
            // `Calc 1: HTRF ratio …` takes the formula stored under `Formula index` `Calc 1`
            for ch in b.channels.iter_mut().filter(|c| c.calculated) {
                if let Some((idx, _)) = ch.label.split_once(':') {
                    ch.formula = formulas.get(idx.trim()).cloned();
                }
            }
            if let Some(w) = b.declared_wells.and_then(dims_for_wells) {
                b.rows = w.0;
                b.cols = w.1;
            }
            ex.blocks.push(b);
        }
    }
    ex.findings.extend(findings);
    if let Some(a) = ex.get("Assay Started").map(str::to_string) {
        ex.set_acquired(&a, None);
    }
    ex
}

fn push(sheet: &Sheet, g: &Grid, block: &mut Block, ch: u32) {
    super::push_grid(sheet, g, block, ch, None);
}

pub(crate) fn parse_kaleido(book: &Book) -> Export {
    let sheet = &book.sheets[0];
    let mut ex = Export::new(Kind::Kaleido, book.container.clone());
    let n = sheet.rows.len();
    let grids = super::titled_grids(sheet);
    let mut measurements: Vec<(String, String)> = Vec::new();
    let mut section = String::new();
    for r in 0..n {
        let cells = sheet.row_texts(r);
        if cells.len() == 1 {
            // a heading; inside `Measurements:` a lone cell is a setting without a value
            let c = cells[0].trim_end_matches(':').trim();
            if c == "Meas" {
                section = "Measurements".into();
            } else if cells[0].ends_with(':') || !matches!(section.as_str(), "Measurements") {
                section = c.to_string();
            }
            continue;
        }
        if grids
            .iter()
            .any(|g| r >= g.grid.header_row && r < g.grid.end_row)
            || matches!(
                section.as_str(),
                "Comments" | "Platemap" | "Plate" | "Analysis"
            )
        {
            continue;
        }
        let first = cells.first().map_or("", String::as_str);
        if first.contains(": ") && !first.ends_with(':') {
            // `Barcode: …, PlateRepeat: 1, WellRepeat: 1` above a results matrix
            for (k, v) in super::colon_pairs(sheet, r) {
                ex.put(format!("Results {k}"), v);
            }
            continue;
        }
        if let Some((k, v)) = kv(sheet, r) {
            match section.as_str() {
                "Measurements" | "Meas" => measurements.push((k, v)),
                _ => ex.put(k, v),
            }
        }
    }
    if !measurements.is_empty() {
        ex.sections.insert(
            "measurements".into(),
            json!(
                measurements
                    .iter()
                    .map(|(k, v)| json!([k, v]))
                    .collect::<Vec<_>>()
            ),
        );
    }
    ex.software_version = ex
        .get("Software version")
        .or_else(|| ex.get("Software Version"))
        .map(|s| s.trim_start_matches("Kaleido ").to_string());
    ex.serial = ex.get("Instrument Serial Number").map(str::to_string);
    ex.protocol = ex.get("Protocol Name").map(str::to_string);
    let started = ex
        .get("Measurement Started")
        .or_else(|| ex.get("Measurement Date"))
        .map(str::to_string);
    if let Some(s) = started {
        ex.set_acquired(&s, None);
    }
    let barcode = ex.get("Barcode").map(str::to_string);
    let wells = ex
        .get("Number of rows")
        .and_then(|r| r.parse::<u32>().ok())
        .zip(
            ex.get("Number of columns")
                .and_then(|c| c.parse::<u32>().ok()),
        );
    let mget = |k: &str| {
        measurements
            .iter()
            .find(|(kk, _)| kk.starts_with(k))
            .map(|(_, v)| v.clone())
    };
    let mut b = Block::new(
        barcode.clone().unwrap_or_else(|| "Plate 1".into()),
        barcode.clone().unwrap_or_else(|| "Plate 1".into()),
    );
    b.barcode = barcode;
    b.read_type = Some(ReadType::Endpoint);
    b.plate_type = ex.get("Plate Type Name").map(str::to_string);
    b.declared_wells = wells.map(|(r, c)| r * c);
    b.started_at.clone_from(&ex.acquired_at);
    for t in &grids {
        let title = t
            .find(|l| l.starts_with("Results for") || l.starts_with("Result for"))
            .unwrap_or("");
        if title.is_empty() {
            continue; // platemap and other layouts
        }
        let label = title
            .trim_start_matches("Results for")
            .trim_start_matches("Result for")
            .trim()
            .to_string();
        let tech = mget("Tech").unwrap_or_else(|| label.clone());
        // The title and `Tech` name the measurement technology (`ABS mono 1`): the mode comes
        // from keywords in that name.
        let mode = match Mode::from_text(&label) {
            Mode::Unknown => Mode::from_text(&tech),
            m => m,
        };
        let mut ch = Channel::derived(label, mode, ModeBasis::Label);
        match mode {
            Mode::Absorbance => {
                ch.wavelength_nm = mget("Excitation wavelength").and_then(|v| first_number(&v));
            }
            Mode::Fluorescence => {
                ch.excitation_nm = mget("Excitation wavelength").and_then(|v| first_number(&v));
                ch.emission_nm = mget("Emission wavelength").and_then(|v| first_number(&v));
            }
            _ => {}
        }
        if let Some(f) = mget("Number of flashes").and_then(|v| crate::sheet::parse_number(&v)) {
            ch.settings.insert("flashes".into(), json!(f));
        }
        if b.line.is_none() {
            b.line = Some(t.grid.header_row + 1);
        }
        let i = b.channel(ch);
        super::push_grid(sheet, &t.grid, &mut b, i, None);
    }
    ex.blocks.push(b);
    ex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn envision_absorbance() {
        let t = "Plate information\nPlate,Repeat,Barcode,Measured height,Chamber temperature at start\n1,1,=\"BC1\",14.35,22.00\nBackground information\nPlate,Label\nResults for A450(1) - channel 1 (A)\n,01,02,03\nA,0.1,0.2,0.3\nB,0.4,0.5,0.6\nBasic assay information\nAssay Started: ,,,,1/15/2024 10:28:00 AM\nLabels:\nA450,,,,2000014\nExc. filter,,,,BFP 450\nFilters:\nBFP 450,,,,216\nFilter type,,,,Emission\nDescription,,,,M450 CWL=450nm BW=10nm Tmin=60%\nInstrument:\nSerial number,,,,1050242\nExported with EnVision Workstation version 1.14.3049.1642\n";
        assert!(sniff_envision(t));
        let ex = parse_envision(&text_book(t.as_bytes()));
        assert_eq!(ex.serial.as_deref(), Some("1050242"));
        assert_eq!(ex.software_version.as_deref(), Some("1.14.3049.1642"));
        let b = &ex.blocks[0];
        assert_eq!(b.barcode.as_deref(), Some("BC1"));
        assert_eq!(b.channels[0].mode, Mode::Absorbance);
        assert_eq!(b.channels[0].wavelength_nm, Some(450.0));
        assert_eq!(b.obs.len(), 6);
        assert_eq!(b.temperature_c, Some(22.0));
    }

    #[test]
    fn envision_bare_matrix() {
        let t = "Plate information\nPlate,Repeat,Barcode\n1,1,=\"\"\nResults for L(1) - channel 1\n1,2,3\n4,5,6\nLabels:\nL,,,,1\n";
        let ex = parse_envision(&text_book(t.as_bytes()));
        assert_eq!(ex.blocks[0].obs.len(), 6);
        assert_eq!(ex.blocks[0].obs[5].row, 1);
        // No keyword in the label and no detector: the mode is unknown, not guessed.
        assert_eq!(ex.blocks[0].channels[0].mode, Mode::Unknown);
        assert_eq!(ex.blocks[0].channels[0].mode_basis, ModeBasis::Undetermined);
    }

    #[test]
    fn modes_from_labels_and_detectors() {
        assert_eq!(label_mode("US LUM 384 (cps)"), Some(Mode::Luminescence));
        assert_eq!(label_mode("A450"), Some(Mode::Absorbance));
        assert_eq!(label_mode("AC HTRF Laser [Eu]"), Some(Mode::Fluorescence));
        assert_eq!(label_mode("CTG readout"), None);
        assert_eq!(
            detector_mode("De=USLum Ex=N/A Em=N/A Wdw=N/A (12)"),
            Some(Mode::Luminescence)
        );
        assert_eq!(
            detector_mode("De=1st Ex=Btm Em=N/A Wdw=N/A (8)"),
            Some(Mode::Absorbance)
        );
        assert_eq!(
            detector_mode("De=2nd Ex=Top Em=Top Wdw=1 (142)"),
            Some(Mode::Fluorescence)
        );
        assert_eq!(bare_label("US LUM 1536 (cps)(1)"), "US LUM 1536 (cps)");
    }

    #[test]
    fn untitled_padded_matrix_takes_the_plate_label() {
        // No results title, no row letters: a 2 x 3 block is no standard plate and is refused,
        // a 8 x 12 block with an empty first field is read, rows without values included.
        let mut t = String::from(
            "Plate information\nPlate,Repeat,Barcode,Group,Label,Measinfo,Measurement date,\n4,1,K1,1,IMPORTED LUM 384 tenth(1),De=USLum Ex=N/A Em=N/A Wdw=N/A (12),9/21/2020 4:58:18 PM,\n\n",
        );
        t.push_str(&format!(",{}\n", [""; 12].join(",")));
        for r in 1..8 {
            t.push_str(&format!(
                ",{}\n",
                (1..=12)
                    .map(|c| (r * 100 + c).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        t.push_str("\nBasic assay information\nAssay ID: ,,,,1\nExported with EnVision Workstation version 1.13.3009.1401\n");
        assert!(sniff_envision(&t));
        let ex = parse_envision(&text_book(t.as_bytes()));
        let b = &ex.blocks[0];
        assert_eq!(b.barcode.as_deref(), Some("K1"));
        assert_eq!(b.obs.len(), 7 * 12);
        assert_eq!((b.obs[0].row, b.obs[0].col), (1, 0));
        assert_eq!(b.channels[0].mode, Mode::Luminescence);
        assert_eq!(b.channels[0].mode_basis, ModeBasis::Detector);
    }

    #[test]
    fn plate_information_after_the_data() {
        let t = "Calculated results: Calc 1: Crosstalk\n;01;02\nA;1;2\nB;3;4\n\nPlate information\nPlate;Repeat;Barcode\n4;1;\n\nResults for LW US LUM 384(1) - channel 0 (CPS)\n;01;02\nA;1;2\nB;3;4\n\nPlate information\nPlate;Repeat;Barcode;Label;Measinfo\n4;1;;LW US LUM 384(1);De=USLum Ex=N/A Em=N/A\n\nBasic assay information\nAssay ID: ;;;;137\nAuto export parameters:\nPlace plate information at;;;;End of plate\nLabels:\nLabel name;;;;LW US LUM 384\nExported with EnVision Workstation version 1.13.3009.1409\n";
        assert!(sniff_envision(t));
        let ex = parse_envision(&text_book(t.as_bytes()));
        assert_eq!(ex.blocks.len(), 1);
        let b = &ex.blocks[0];
        assert_eq!(b.name, "Plate 4");
        assert_eq!(b.channels.len(), 2);
        assert!(b.channels[0].calculated);
        assert_eq!(b.channels[1].mode, Mode::Luminescence);
        assert_eq!(b.obs.len(), 8);
    }
}
