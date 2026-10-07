//! Agilent BioTek Gen5 text/XLSX exports (Synergy, Cytation, Epoch, ELx readers).
//!
//! Layout (docs/formats/plate-readers.md, "Gen5"): header `key<TAB>value` lines up to
//! `Procedure Details`; the procedure (plate type, `Read` steps with wavelengths or filter
//! sets, kinetic loop); then blank-line separated sections: `Results` matrices whose rows
//! carry a trailing `read:wavelength` label and one continuation line per further read,
//! `Layout`, `Actual Temperature:`, kinetic `Time` tables and spectral `Wavelength` tables.

use std::collections::BTreeMap;

use openreadout_core::model::Finding;
use serde_json::{Value, json};

use crate::datetime::parse_duration;
use crate::grid::{column_number, parse_row_label, parse_well, well_name};
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType, first_number, nonempty};
use crate::sheet::{Book, Sheet};

/// Does this text look like a Gen5 export?
pub(crate) fn sniff(text: &str) -> bool {
    let mut version = false;
    let mut markers = 0;
    for l in text.lines().take(80) {
        let l = l.trim_start_matches('\u{feff}');
        if l.starts_with("Software Version") {
            version = true;
        }
        if l.starts_with("Procedure Details")
            || l.starts_with("Plate Number")
            || l.starts_with("Reader Type:")
            || l.starts_with("Reader Serial Number:")
        {
            markers += 1;
        }
    }
    version && markers >= 2 || sniff_headerless(text)
}

/// A Gen5 export written without its file header (Gen5's export options can leave out the
/// `Software Version` … `Reading Type` block, and the procedure): the text starts at a section.
/// Recognised by Gen5's own section markers: a `Layout` matrix whose rows carry the trailing
/// `Well ID` label, a `Procedure Details` / `Procedure Summary` block followed by a `Results`
/// section, or a `Curve Name<TAB>Curve Formula` fit table.
pub(crate) fn sniff_headerless(text: &str) -> bool {
    let mut layout = false;
    let mut well_id = false;
    let mut procedure = false;
    let mut results = false;
    let mut curve = false;
    for l in text.lines().take(200) {
        let l = l.trim_start_matches('\u{feff}');
        let t = l.trim();
        match t {
            "Layout" => layout = true,
            "Results" => results = true,
            "Procedure Details" | "Procedure Summary" => procedure = true,
            _ => {}
        }
        if l.trim_end().rsplit('\t').next() == Some("Well ID") && l.contains('\t') {
            well_id = true;
        }
        if l.starts_with("Curve Name\tCurve Formula") {
            curve = true;
        }
    }
    (layout && well_id) || (procedure && results) || curve
}

/// A filter set / wavelength of one `Read` step.
#[derive(Debug, Clone, Default)]
struct Spec {
    excitation: Option<String>,
    emission: Option<String>,
    settings: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
struct Read {
    name: Option<String>,
    mode: Mode,
    read_type: ReadType,
    wavelengths: Vec<f64>,
    specs: Vec<Spec>,
    settings: BTreeMap<String, Value>,
    spectrum: Option<(f64, f64, f64)>,
}

fn read_type_of(s: &str) -> Option<(Mode, ReadType)> {
    let l = s.to_ascii_lowercase();
    let mode = Mode::from_text(s);
    if mode == Mode::Unknown {
        return None;
    }
    let t = if l.contains("spectrum") {
        ReadType::Spectrum
    } else if l.contains("kinetic") {
        ReadType::Kinetic
    } else if l.contains("endpoint") || l.contains("scan") {
        ReadType::Endpoint
    } else {
        return None;
    };
    Some((mode, t))
}

/// `Key: Value,  Key: Value` pairs of a procedure detail line.
fn pairs(s: &str) -> Vec<(String, String)> {
    s.split(",  ")
        .flat_map(|p| p.split(", "))
        .filter_map(|p| p.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

fn normalized_setting(key: &str, value: &str, into: &mut BTreeMap<String, Value>) {
    let num = || first_number(value).map_or_else(|| json!(value), |n| json!(n));
    match key {
        "Gain" => into.insert("gain".into(), json!(value)),
        "Optics" => into.insert("optics".into(), json!(value)),
        "Mirror" => into.insert("mirror".into(), json!(value)),
        "Read Speed" => into.insert("read_speed".into(), json!(value)),
        "Delay" => into.insert("delay".into(), json!(value)),
        "Measurements/Data Point" => into.insert("measurements_per_point".into(), num()),
        "Read Height" => into.insert("read_height_mm".into(), num()),
        "Integration Time" => into.insert("integration_time".into(), json!(value)),
        "Light Source" => into.insert("light_source".into(), json!(value)),
        "Lamp Energy" => into.insert("lamp_energy".into(), json!(value)),
        "Pathlength Correction" => into.insert("pathlength_correction".into(), json!(value)),
        _ => into.insert(key.to_string(), json!(value)),
    };
}

struct Procedure {
    lines: Vec<String>,
    plate_type: Option<String>,
    reads: Vec<Read>,
    kinetic: Option<Value>,
}

fn parse_procedure(sheet: &Sheet, start: usize, end: usize) -> (Procedure, usize) {
    let mut p = Procedure {
        lines: Vec::new(),
        plate_type: None,
        reads: Vec::new(),
        kinetic: None,
    };
    let mut r = start;
    while r < end && sheet.row_is_blank(r) {
        r += 1;
    }
    while r < end && !sheet.row_is_blank(r) {
        let row = sheet.rows.get(r).map_or(&[][..], Vec::as_slice);
        p.lines.push(
            row.iter()
                .map(crate::sheet::Cell::text)
                .collect::<Vec<_>>()
                .join("\t")
                .trim_end()
                .to_string(),
        );
        let key = sheet.text(r, 0);
        let val = sheet.text(r, 1);
        match key.as_str() {
            "Plate Type" => p.plate_type = nonempty(&val),
            "Start Kinetic" => {
                let mut k = serde_json::Map::new();
                k.insert("description".into(), json!(val));
                for part in val.split(", ") {
                    if let Some(rt) = part.strip_prefix("Runtime ") {
                        if let Some(s) = parse_duration(rt.split_whitespace().next().unwrap_or(""))
                        {
                            k.insert("runtime_s".into(), json!(s));
                        }
                    } else if let Some(iv) = part.strip_prefix("Interval ") {
                        if let Some(s) = parse_duration(iv.split_whitespace().next().unwrap_or(""))
                        {
                            k.insert("interval_s".into(), json!(s));
                        }
                    } else if let Some(n) = part.strip_suffix(" Reads")
                        && let Ok(n) = n.trim().parse::<u32>()
                    {
                        k.insert("reads".into(), json!(n));
                    }
                }
                p.kinetic = Some(Value::Object(k));
            }
            "Read" => {
                let (name, mode, t) = match read_type_of(&val) {
                    Some((m, t)) => (None, m, t),
                    None => (nonempty(&val), Mode::Unknown, ReadType::Endpoint),
                };
                p.reads.push(Read {
                    name,
                    mode,
                    read_type: t,
                    wavelengths: Vec::new(),
                    specs: Vec::new(),
                    settings: BTreeMap::new(),
                    spectrum: None,
                });
            }
            "" => {
                if let Some(read) = p.reads.last_mut() {
                    detail(read, val.trim());
                }
            }
            _ => {}
        }
        r += 1;
    }
    if p.kinetic.is_some() {
        for read in &mut p.reads {
            if read.read_type == ReadType::Endpoint {
                read.read_type = ReadType::Kinetic;
            }
        }
    }
    (p, r)
}

fn detail(read: &mut Read, d: &str) {
    if read.mode == Mode::Unknown
        && let Some((m, t)) = read_type_of(d)
    {
        read.mode = m;
        read.read_type = t;
        return;
    }
    if let Some(w) = d.strip_prefix("Wavelengths:") {
        read.wavelengths = w
            .split(',')
            .filter_map(|x| x.trim().parse::<f64>().ok())
            .collect();
        return;
    }
    if d.starts_with("Filter Set") {
        read.specs.push(Spec::default());
        return;
    }
    if d.starts_with("Start:") && d.contains("Stop:") {
        let v = crate::model::numbers(d);
        if v.len() >= 3 {
            read.spectrum = Some((v[0], v[1], v[2]));
        }
        return;
    }
    let kv = pairs(d);
    if kv.is_empty() {
        if let Some(a) = read
            .settings
            .entry("notes".into())
            .or_insert_with(|| json!([]))
            .as_array_mut()
        {
            a.push(json!(d));
        }
        return;
    }
    let in_filter = kv.iter().any(|(k, _)| k == "Excitation" || k == "Emission");
    if in_filter && read.specs.is_empty() {
        read.specs.push(Spec::default());
    }
    for (k, v) in kv {
        match k.as_str() {
            "Excitation" => {
                if let Some(s) = read.specs.last_mut() {
                    s.excitation = Some(v);
                }
            }
            "Emission" => {
                if let Some(s) = read.specs.last_mut() {
                    s.emission = Some(v);
                }
            }
            "Optics" | "Mirror" | "Gain" if !read.specs.is_empty() => {
                if let Some(s) = read.specs.last_mut() {
                    normalized_setting(&k, &v, &mut s.settings);
                }
            }
            _ => normalized_setting(&k, &v, &mut read.settings),
        }
    }
}

/// Map a data label (`od:600`, `DAPI/GFP:360/40,460/40`, `460/40`, `LUM:Lum`, `normDAPI`) to a
/// channel; labels that name no procedure read are values calculated by Gen5.
fn channel_for(label: &str, reads: &[Read]) -> Channel {
    let label = strip_read_prefix(label);
    let (read, spec) = if let Some((name, spec)) = label.rsplit_once(':') {
        match reads.iter().find(|r| r.name.as_deref() == Some(name)) {
            Some(r) => (Some(r), spec.to_string()),
            None => (None, String::new()),
        }
    } else {
        {
            let unnamed: Vec<&Read> = reads.iter().filter(|r| r.name.is_none()).collect();
            let plain = label
                .chars()
                .all(|c| c.is_ascii_digit() || "./, ".contains(c))
                || ["Lum", "Spectrum", "Alpha"].contains(&label);
            match (unnamed.as_slice(), plain) {
                ([r], true) => (Some(*r), label.to_string()),
                (many, true) if !many.is_empty() => {
                    // several unnamed reads: the one whose filter set is the label's
                    // (`485/20,528/20`), else the first whose mode fits the label
                    let fl = label.contains(',');
                    let filters = label
                        .split_once(',')
                        .map(|(ex, em)| (first_number(ex), first_number(em)))
                        .filter(|(ex, em)| ex.is_some() && em.is_some());
                    let r = many
                        .iter()
                        .find(|r| {
                            filters.is_some_and(|(ex, em)| {
                                r.specs.iter().any(|s| {
                                    s.excitation.as_deref().and_then(first_number) == ex
                                        && s.emission.as_deref().and_then(first_number) == em
                                })
                            })
                        })
                        .or_else(|| many.iter().find(|r| (r.mode == Mode::Fluorescence) == fl))
                        .unwrap_or(&many[0]);
                    (Some(*r), label.to_string())
                }
                _ => (None, String::new()),
            }
        }
    };
    let Some(read) = read else {
        let mut c = Channel::new(label, Mode::Unknown);
        c.calculated = true;
        return c;
    };
    let mut c = Channel::new(label, read.mode);
    c.settings = read.settings.clone();
    if let Some(n) = &read.name {
        c.settings.insert("step".into(), json!(n));
    }
    match read.mode {
        Mode::Absorbance => {
            c.wavelength_nm = first_number(&spec);
            c.unit = Some("OD".into());
        }
        Mode::Fluorescence | Mode::Alpha => {
            let parts: Vec<&str> = spec.split(',').collect();
            if parts.len() == 2 {
                c.excitation_nm = first_number(parts[0]);
                c.emission_nm = first_number(parts[1]);
            } else {
                c.emission_nm = first_number(&spec);
            }
            c.unit = Some("RFU".into());
            if let Some(s) = read.specs.iter().find(|s| {
                s.excitation.as_deref().map(first_number) == Some(c.excitation_nm)
                    && s.emission.as_deref().map(first_number) == Some(c.emission_nm)
            }) {
                c.settings.extend(s.settings.clone());
                if let Some(bw) = s.excitation.as_deref().and_then(bandwidth) {
                    c.settings
                        .insert("excitation_bandwidth_nm".into(), json!(bw));
                }
                if let Some(bw) = s.emission.as_deref().and_then(bandwidth) {
                    c.settings.insert("emission_bandwidth_nm".into(), json!(bw));
                }
            }
        }
        Mode::Luminescence => {
            c.emission_nm = first_number(&spec);
            c.unit = Some("RLU".into());
            if let Some(s) = read.specs.first() {
                c.settings.extend(s.settings.clone());
            }
        }
        Mode::Unknown => {}
    }
    c
}

fn bandwidth(s: &str) -> Option<f64> {
    s.split_once('/').and_then(|(_, b)| first_number(b))
}

fn strip_read_prefix(label: &str) -> &str {
    if let Some(rest) = label.strip_prefix("Read ")
        && let Some((n, tail)) = rest.split_once(':')
        && n.chars().all(|c| c.is_ascii_digit())
    {
        return tail;
    }
    label
}

/// Does this worksheet start with a Gen5 export (its header, or a section of a header-less one)?
fn sheet_holds_export(sheet: &Sheet) -> bool {
    let head: String = (0..sheet.rows.len().min(200))
        .map(|r| {
            sheet.rows[r]
                .iter()
                .map(crate::sheet::Cell::text)
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n");
    sniff(&head)
}

/// Where a plate comes from: its worksheet (named when the workbook holds several exports) and
/// its position among the plates.
#[derive(Clone, Copy)]
struct Origin<'a> {
    sheet: Option<&'a str>,
    /// Plates read before this one, over every sheet.
    index: usize,
    /// Plates before this one in the same sheet.
    in_sheet: usize,
}

impl Origin<'_> {
    /// Prefix of this plate's header keys and section names (none for the first plate of a
    /// one-export file).
    fn prefix(&self) -> Option<String> {
        match (self.sheet, self.in_sheet) {
            (Some(s), 0) => Some(s.to_string()),
            (Some(s), n) => Some(format!("{s} / plate {}", n + 1)),
            (None, 0) => None,
            (None, n) => Some(format!("plate {}", n + 1)),
        }
    }
}

pub(crate) fn parse(book: &Book, file_name: &str) -> Export {
    let mut ex = Export::new(Kind::Gen5, book.container.clone());
    // A workbook can hold several exports, one per worksheet.
    let mut sheets: Vec<&Sheet> = book
        .sheets
        .iter()
        .filter(|s| sheet_holds_export(s))
        .collect();
    if sheets.is_empty() {
        sheets.extend(book.sheets.first());
    }
    let several = sheets.len() > 1;
    let barcode = barcode_from_name(file_name);
    let mut index = 0;
    for sheet in &sheets {
        ex.sheets_read.push(sheet.name.clone());
        let name = several.then_some(sheet.name.as_str());
        index += parse_sheet(sheet, name, index, &mut ex, barcode.as_deref());
    }
    if several {
        ex.notes.push(format!(
            "the workbook holds {} Gen5 exports, one per worksheet; each plate is one table named after its sheet",
            sheets.len()
        ));
    }
    ex
}

/// The plates of one sheet; returns how many there are.
fn parse_sheet(
    sheet: &Sheet,
    name: Option<&str>,
    index: usize,
    ex: &mut Export,
    barcode: Option<&str>,
) -> usize {
    let starts: Vec<usize> = (0..sheet.rows.len())
        .filter(|&r| sheet.text(r, 0).trim_start_matches('\u{feff}') == "Software Version")
        .collect();
    let origin = |in_sheet| Origin {
        sheet: name,
        index: index + in_sheet,
        in_sheet,
    };
    if starts.is_empty() {
        // no `Software Version` line: an export without its file header
        parse_plate(sheet, 0, sheet.rows.len(), ex, barcode, origin(0), true);
        ex.notes.push(
            "Gen5 export without its file header: reader, serial number, date and (unless the export keeps it) the procedure are not recorded; reads are named from the result labels".into(),
        );
        return 1;
    }
    for (i, &s) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(sheet.rows.len());
        parse_plate(sheet, s, end, ex, barcode, origin(i), false);
    }
    if starts.len() > 1 {
        ex.notes.push(format!(
            "{} plates in one export{}; each is one table",
            starts.len(),
            name.map(|n| format!(" (worksheet {n:?})"))
                .unwrap_or_default()
        ));
    }
    starts.len()
}

/// Gen5 export names often start `yymmdd_hhmmss_<barcode>_…` (pattern from allotropy).
fn barcode_from_name(name: &str) -> Option<String> {
    let parts: Vec<&str> = name.split('_').collect();
    (parts.len() >= 4
        && parts[0].len() == 6
        && parts[1].len() == 6
        && parts[0]
            .chars()
            .chain(parts[1].chars())
            .all(|c| c.is_ascii_digit())
        && !parts[2].is_empty())
    .then(|| parts[2].to_string())
}

/// Is row `r` the start of the procedure block?
fn is_procedure(sheet: &Sheet, r: usize) -> bool {
    matches!(
        sheet.text(r, 0).trim_start_matches('\u{feff}'),
        "Procedure Details" | "Procedure Summary"
    )
}

#[allow(clippy::too_many_lines)]
fn parse_plate(
    sheet: &Sheet,
    start: usize,
    end: usize,
    ex: &mut Export,
    barcode: Option<&str>,
    origin: Origin,
    headerless: bool,
) {
    let index = origin.index;
    let mut r = start;
    let mut header: Vec<(String, String)> = Vec::new();
    if headerless {
        // the procedure, when kept, comes before the first section
        r = (start..end)
            .take_while(|&i| {
                !matches!(
                    sheet.text(i, 0).trim_start_matches('\u{feff}'),
                    "Layout" | "Results"
                )
            })
            .find(|&i| is_procedure(sheet, i))
            .unwrap_or(start);
    } else {
        while r < end && !is_procedure(sheet, r) {
            let k = sheet.text(r, 0).trim_start_matches('\u{feff}').to_string();
            if !k.is_empty() {
                header.push((k, sheet.text(r, 1)));
            }
            r += 1;
        }
    }
    let get = |key: &str| {
        header
            .iter()
            .find(|(k, _)| k.trim_end_matches(':') == key)
            .and_then(|(_, v)| nonempty(v))
    };
    if index == 0 {
        ex.software_version = get("Software Version");
        ex.model = get("Reader Type").filter(|v| v != "n/a");
        ex.serial = get("Reader Serial Number").filter(|v| v != "n/a" && v != "Unknown");
        ex.protocol = get("Protocol File Path");
        ex.experiment = get("Experiment File Path");
        if let Some(d) = get("Date") {
            ex.set_acquired(&d, get("Time").as_deref());
        }
    }
    let prefix = origin.prefix();
    for (k, v) in &header {
        let key = match &prefix {
            None => k.clone(),
            Some(p) => format!("{p} / {k}"),
        };
        ex.put(key, v.clone());
    }
    let plate_number =
        get("Plate Number").unwrap_or_else(|| format!("Plate {}", origin.in_sheet + 1));
    let mut block = Block::new(
        match origin.sheet {
            Some(s) => format!("{plate_number} ({s})"),
            None => plate_number.clone(),
        },
        barcode.map_or_else(|| plate_number.clone(), str::to_string),
    );
    block.sheet = (!sheet.name.is_empty()).then(|| sheet.name.clone());
    block.barcode = barcode.map(str::to_string);
    block.decimal_comma = true;
    block.line = Some(start + 1);
    block
        .extra
        .insert("plate_number".into(), json!(plate_number));
    if let Some(rt) = get("Reading Type") {
        block.extra.insert("reading_type".into(), json!(rt));
    }
    if let (Some(d), t) = (get("Date"), get("Time"))
        && let Some((iso, _)) = crate::datetime::combine(&d, t.as_deref())
    {
        block.started_at = Some(iso);
    }
    if r >= end && !headerless {
        block.findings.push(Finding::error(
            "missing_section",
            format!("{}: no `Procedure Details` section", block.name),
        ));
        ex.blocks.push(block);
        return;
    }
    let (mut proc_, mut r) = if headerless && !is_procedure(sheet, r) {
        (
            Procedure {
                lines: Vec::new(),
                plate_type: None,
                reads: Vec::new(),
                kinetic: None,
            },
            r,
        )
    } else {
        parse_procedure(sheet, r + 1, end)
    };
    if headerless && proc_.reads.is_empty() {
        proc_.reads = reads_from_labels(sheet, r, end);
        if !proc_.reads.is_empty() {
            block.extra.insert(
                "reads_inferred_from_labels".into(),
                json!(
                    proc_
                        .reads
                        .iter()
                        .map(|x| x.name.clone().unwrap_or_else(|| x
                            .wavelengths
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(",")))
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
    block.plate_type = proc_.plate_type.clone();
    block.declared_wells = proc_
        .plate_type
        .as_deref()
        .and_then(first_number)
        .map(|n| n as u32)
        .filter(|n| crate::grid::dims_for_wells(*n).is_some());
    if let Some(k) = &proc_.kinetic {
        block.extra.insert("kinetic".into(), k.clone());
    }
    let key = match &prefix {
        None => "procedure".to_string(),
        Some(p) => format!("procedure ({p})"),
    };
    if !proc_.lines.is_empty() || !headerless {
        ex.sections.insert(key, json!(proc_.lines));
    }
    let reads = proc_.reads;
    if reads.is_empty() && !headerless {
        block.findings.push(Finding::warning(
            "no_read_step",
            format!("{}: the procedure has no `Read` step", block.name),
        ));
    }
    let mut layout: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut temps: Vec<f64> = Vec::new();
    let mut read_types = Vec::new();
    let mut last_title: Option<String> = None;
    while r < end {
        if sheet.row_is_blank(r) {
            r += 1;
            continue;
        }
        let t0 = sheet.text(r, 0);
        let title = if t0.is_empty() {
            sheet.text(r, 1)
        } else {
            t0.clone()
        };
        if title.starts_with("Actual Temperature") {
            if let Some(v) = sheet.cell(r, 1).number() {
                temps.push(v);
            }
            r += 1;
            continue;
        }
        // Matrix: header of column numbers right here or after the title line. Text exports
        // put the row letters in the first column; Gen5's Excel exports one column further
        // right (letters in B, column numbers from C).
        let header_at = |row: usize| {
            (1..=2).find(|&c0| {
                column_number(sheet.cell(row, c0)) == Some(1)
                    && (0..c0).all(|c| sheet.text(row, c).is_empty())
            })
        };
        if let Some(c0) = header_at(r) {
            r = parse_matrix(
                sheet,
                (r, c0),
                end,
                last_title.take().unwrap_or_else(|| "Results".into()),
                &reads,
                &mut block,
                &mut layout,
            );
            continue;
        }
        // Kinetic and spectral tables: `Time` / `Wavelength` in column A (text exports) or B
        // (Excel exports, column A empty).
        if title == "Time" || title == "Wavelength" {
            let label = last_title.take().unwrap_or_default();
            let c0 = usize::from(t0.is_empty());
            let (next, kind) = parse_table(sheet, (r, c0), end, &label, &reads, &mut block);
            read_types.push(kind);
            r = next;
            continue;
        }
        // A title line: the next non-blank line holds the section. A line with nothing in its
        // first two columns (cells a depositor added to the sheet) is not a title.
        if !title.is_empty() {
            last_title = Some(title);
        }
        r += 1;
    }
    if !layout.is_empty() {
        block.extra.insert("layout".into(), json!(layout));
    }
    if let Some(t) = temps.first() {
        block.temperature_c = Some(*t);
        if temps.len() > 1 {
            block
                .extra
                .insert("actual_temperatures_c".into(), json!(temps));
        }
    }
    let rt = if read_types.contains(&ReadType::Spectrum) {
        ReadType::Spectrum
    } else if read_types.contains(&ReadType::Kinetic) {
        ReadType::Kinetic
    } else {
        reads.first().map_or(ReadType::Endpoint, |r| r.read_type)
    };
    block.read_type = Some(rt);
    // Without the procedure, the reads' modes come from their labels (`reads_from_labels`).
    if block.extra.contains_key("reads_inferred_from_labels") {
        for c in block.channels.iter_mut().filter(|c| !c.calculated) {
            c.set_mode(c.mode, crate::model::ModeBasis::Label);
        }
    }
    if block.obs.is_empty() {
        block.findings.push(Finding::warning(
            "no_plate_data",
            format!(
                "{}: no results matrix, kinetic or spectrum table",
                block.name
            ),
        ));
    }
    ex.blocks.push(block);
}

/// Without the procedure (a header-less export), the reads are named by the result labels
/// Gen5 writes after each matrix row or above each kinetic table: `<read>:<spec>` (optionally
/// `Read <n>:` first). The spec says the mode: `485,528` excitation,emission → fluorescence,
/// `Lum` → luminescence, a single wavelength → absorbance (Gen5 labels fluorescence reads with
/// both filters). Labels Gen5 derives (`Blank <read>`, `[Concentration]`, `Mean V`, ...) name no
/// read and stay calculated channels.
fn reads_from_labels(sheet: &Sheet, start: usize, end: usize) -> Vec<Read> {
    let mut out: Vec<Read> = Vec::new();
    for r in start..end {
        let Some(row) = sheet.rows.get(r) else { break };
        let filled: Vec<&crate::sheet::Cell> = row.iter().filter(|c| !c.is_blank()).collect();
        let Some(last) = filled.last() else {
            continue;
        };
        // a title line (one cell) or the trailing label of a matrix row (a text cell)
        let title = filled.len() == 1;
        if !title && last.number().is_some() {
            continue;
        }
        let text = last.text();
        let label = strip_read_prefix(text.trim());
        let (name, spec) = match label.rsplit_once(':') {
            Some((n, sp)) => (n.trim(), sp.trim()),
            // an unnamed read's title: its wavelength(s) alone (`420`, `485,528`)
            None if title => ("", label.trim()),
            None => continue,
        };
        if name.is_empty() && out.iter().any(|x| x.name.is_none()) {
            continue;
        }
        if (name.is_empty() && label.contains(':'))
            || name.starts_with("Blank ")
            || name.starts_with('[')
            || name.contains('\t')
            || out.iter().any(|x| x.name.as_deref() == Some(name))
        {
            continue;
        }
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        let numeric = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit() || c == '.');
        let mode = match parts.as_slice() {
            [a, b] if numeric(a) && numeric(b) => Mode::Fluorescence,
            [a] if numeric(a) => Mode::Absorbance,
            [a] if a.eq_ignore_ascii_case("lum") => Mode::Luminescence,
            _ => continue,
        };
        out.push(Read {
            name: (!name.is_empty()).then(|| name.to_string()),
            mode,
            read_type: ReadType::Endpoint,
            wavelengths: if mode == Mode::Absorbance {
                parts.iter().filter_map(|p| p.parse().ok()).collect()
            } else {
                Vec::new()
            },
            specs: Vec::new(),
            settings: BTreeMap::new(),
            spectrum: None,
        });
    }
    out
}

fn parse_matrix(
    sheet: &Sheet,
    (header, c0): (usize, usize),
    end: usize,
    title: String,
    reads: &[Read],
    block: &mut Block,
    layout: &mut BTreeMap<String, BTreeMap<String, String>>,
) -> usize {
    let mut n = 0u32;
    while column_number(sheet.cell(header, c0 + n as usize)) == Some(n + 1) {
        n += 1;
    }
    let label_col = c0 + n as usize;
    let mut r = header + 1;
    let mut current: Option<u32> = None;
    let is_layout = title == "Layout";
    while r < end && !sheet.row_is_blank(r) {
        let lab = sheet.text(r, c0 - 1);
        if lab.is_empty() {
            if current.is_none() {
                break;
            }
        } else if let Some(pr) = parse_row_label(&lab, false) {
            current = Some(pr);
        } else {
            break;
        }
        let row = current.unwrap_or(0);
        let trailing = sheet.text(r, label_col);
        let label = if trailing.is_empty() {
            title.clone()
        } else {
            trailing
        };
        if is_layout {
            let m = layout.entry(label).or_default();
            for c in 0..n {
                let t = sheet.text(r, c0 + c as usize);
                if !t.is_empty() {
                    m.insert(well_name(row, c), t);
                }
            }
        } else {
            let ch = block.channel(channel_for(&label, reads));
            for c in 0..n {
                let cell = sheet.cell(r, c0 + c as usize).clone();
                block.push_cell(row, c, ch, None, None, &cell);
            }
        }
        r += 1;
    }
    r
}

/// A kinetic `Time` table or a spectral `Wavelength` table: wells as columns. `c0` is the
/// column of `Time` / `Wavelength`: 0 in text exports, 1 in Excel exports.
fn parse_table(
    sheet: &Sheet,
    (header, c0): (usize, usize),
    end: usize,
    label: &str,
    reads: &[Read],
    block: &mut Block,
) -> (usize, ReadType) {
    let key_title = sheet.text(header, c0);
    let spectral = key_title == "Wavelength";
    let width = sheet.rows.get(header).map_or(0, Vec::len);
    let mut wells = Vec::new();
    let mut temp_col = None;
    for c in c0 + 1..width {
        let t = sheet.text(header, c);
        if let Some(w) = parse_well(&t) {
            wells.push((c, w));
        } else if c == c0 + 1 && !t.is_empty() {
            temp_col = Some(c);
        }
    }
    // The title line names the read. Failing that, the temperature column's header repeats
    // the title (`T° 600`, `T° Read 2:485/20,528/20`).
    let mut ch = channel_for(label, reads);
    if ch.calculated
        && let Some(h) = temp_col.map(|c| sheet.text(header, c))
        && let Some((_, title)) = h.split_once(char::is_whitespace)
    {
        let alt = channel_for(title.trim(), reads);
        if !alt.calculated {
            ch = alt;
        }
    }
    if ch.calculated && label.is_empty() {
        // unlabelled table: the only read
        if let [only] = reads {
            ch = channel_for(only.name.as_deref().unwrap_or(""), reads);
            ch.calculated = false;
            ch.mode = only.mode;
        }
    }
    let ch = block.channel(ch);
    let mut temps = Vec::new();
    let mut r = header + 1;
    while r < end && !sheet.row_is_blank(r) {
        // The table ends at a title left of it or at the next table's header, even without a
        // blank line between them (a depositor may have filled the blank lines).
        if (0..c0).any(|c| !sheet.cell(r, c).is_blank()) {
            break;
        }
        let key = sheet.text(r, c0);
        if key == key_title {
            break;
        }
        let has_values = wells.iter().any(|(c, _)| !sheet.cell(r, *c).is_blank());
        if has_values {
            let (time, wl) = if spectral {
                (None, crate::sheet::parse_number(&key))
            } else {
                (duration_s(sheet.cell(r, c0)), None)
            };
            if let Some(tc) = temp_col
                && let Some(t) = sheet.cell(r, tc).number()
            {
                temps.push(t);
            }
            for &(c, (pr, pc)) in &wells {
                let cell = sheet.cell(r, c).clone();
                block.push_cell(pr, pc, ch, time, wl, &cell);
            }
        }
        r += 1;
    }
    if !temps.is_empty() && !spectral {
        block
            .extra
            .insert("kinetic_temperatures_c".into(), json!(temps));
    }
    (
        r,
        if spectral {
            ReadType::Spectrum
        } else {
            ReadType::Kinetic
        },
    )
}

/// A kinetic table's time: text (`0:04:22`), or in an Excel export a time value, which is a
/// fraction of a day (above 1 from 24 h on).
fn duration_s(cell: &crate::sheet::Cell) -> Option<f64> {
    match cell {
        crate::sheet::Cell::Date(days) if days.is_finite() && *days >= 0.0 => {
            Some((days * 86_400_000.0).round() / 1000.0)
        }
        other => parse_duration(&other.trimmed()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    const MULTI: &str = "Software Version\t3.15.15\n\nPlate Number\tPlate 1\nDate\t6/12/2024\nTime\t5:47:49 PM\nReader Type:\tSynergy H1\nReader Serial Number:\t12345678\n\nProcedure Details\n\nPlate Type\t96 WELL PLATE (Use plate lid)\nRead\tod\n\tAbsorbance Endpoint\n\tWavelengths:  600\nRead\tfluor\n\tFluorescence Endpoint\n\tFilter Set 1\n\t    Excitation: 579,  Emission: 616\n\t    Optics: Top,  Gain: extended\n\nActual Temperature:\t22.8\n\nResults\n\t1\t2\nA\t0.052\tOVRFLW\tod:600\n\t75\t60\tfluor:579,616\n\t1\t2\tnorm\nB\t0.1\t0.2\tod:600\n";

    #[test]
    fn multi_read() {
        assert!(sniff(MULTI));
        let ex = parse(&text_book(MULTI.as_bytes()), "x.txt");
        assert_eq!(ex.model.as_deref(), Some("Synergy H1"));
        assert_eq!(ex.acquired_at.as_deref(), Some("2024-06-12T17:47:49"));
        let b = &ex.blocks[0];
        assert_eq!(b.channels.len(), 3);
        assert_eq!(b.channels[0].mode, Mode::Absorbance);
        assert_eq!(b.channels[0].wavelength_nm, Some(600.0));
        assert_eq!(b.channels[1].excitation_nm, Some(579.0));
        assert_eq!(b.channels[1].emission_nm, Some(616.0));
        assert!(b.channels[2].calculated);
        assert_eq!(b.obs.len(), 8);
        assert!(b.obs[1].value.is_nan());
        assert_eq!(b.obs[1].text.as_deref(), Some("OVRFLW"));
        assert_eq!(b.temperature_c, Some(22.8));
        assert_eq!(b.declared_wells, Some(96));
    }

    #[test]
    fn kinetic() {
        let t = "Software Version\t3.0.1\nPlate Number\tPlate 2\nReader Type:\tX\n\nProcedure Details\n\nPlate Type\tGeneric\nStart Kinetic\tRuntime 1:00:00 (HH:MM:SS), Interval 0:04:00, 3 Reads\n    Read\tAbsorbance Endpoint\n\tWavelengths:  600\nEnd Kinetic\n\n600\n\nTime\tT\u{221e} 600\tA1\tA2\n0:00:22\t30.0\t-0.066\t-0.068\n0:04:22\t30.1\t-0.067\t-0.069\n0:00:00\n";
        let ex = parse(&text_book(t.as_bytes()), "241001_101010_BAR1_x.txt");
        let b = &ex.blocks[0];
        assert_eq!(b.plate, "BAR1");
        assert_eq!(b.read_type, Some(ReadType::Kinetic));
        assert_eq!(b.obs.len(), 4);
        assert_eq!(b.channels[0].wavelength_nm, Some(600.0));
        assert_eq!(b.obs[2].time_s, Some(262.0));
        assert_eq!(b.extra["kinetic"]["interval_s"], json!(240.0));
    }

    /// Gen5 export options can leave the file header out: the text starts at `Layout`.
    const HEADERLESS: &str = "Layout\n\t1\t2\t3\nA\tSTD1\tSTD2\tBLK\tWell ID\n\t100\t50\t\tConc/Dil\n\nResults\n\t1\t2\t3\nA\t1.2\t0.7\t0.1\tbca:562\n\t1.1\t0.6\t0.0\tBlank bca:562\n\t100.1\t49.8\t\t[Concentration]\n\nStdCurve Fitting Results\n\nCurve Name\tCurve Formula\tA\tB\tR2\tFit F Prob\nStdCurve\tY=A*X+B\t0.011\t0.0\t1\t?????\n";

    #[test]
    fn headerless_export() {
        assert!(sniff(HEADERLESS));
        let ex = parse(&text_book(HEADERLESS.as_bytes()), "plate.txt");
        assert!(
            ex.notes
                .iter()
                .any(|n| n.contains("without its file header"))
        );
        let b = &ex.blocks[0];
        assert!(b.findings.is_empty(), "{:?}", b.findings);
        // the layout is kept
        assert_eq!(b.extra["layout"]["Well ID"]["A1"], json!("STD1"));
        assert_eq!(b.extra["layout"]["Conc/Dil"]["A2"], json!("50"));
        // the read is named from its label; derived rows stay calculated
        assert_eq!(b.extra["reads_inferred_from_labels"], json!(["bca"]));
        let raw = &b.channels[0];
        assert_eq!(raw.mode, Mode::Absorbance);
        assert_eq!(raw.wavelength_nm, Some(562.0));
        assert!(!raw.calculated);
        assert!(b.channels[1].calculated && b.channels[2].calculated);
        assert_eq!(b.obs.len(), 8);
    }

    #[test]
    fn headerless_with_procedure_and_unnamed_kinetic_read() {
        let t = "Procedure Details\n\nPlate Type\t96 WELL PLATE\nStart Kinetic\tRuntime 0:10:00 (HH:MM:SS), Interval 0:05:00, 3 Reads\n    Read\tAbsorbance Endpoint\n\tWavelengths:  420\nEnd Kinetic\n\nResults\n\n420\n\nTime\tT\u{b0} 420\tA1\tA2\n0:00:00\t25.0\t0.1\t0.2\n0:05:00\t25.0\t0.2\t0.3\n";
        assert!(sniff(t));
        let ex = parse(&text_book(t.as_bytes()), "x.txt");
        let b = &ex.blocks[0];
        assert_eq!(b.declared_wells, Some(96));
        assert_eq!(b.read_type, Some(ReadType::Kinetic));
        assert_eq!(b.channels[0].wavelength_nm, Some(420.0));
        assert_eq!(b.obs.len(), 4);
        // no procedure: the unnamed read is named by its wavelength title
        let bare = &t[t.find("Results").unwrap()..];
        assert!(!sniff(bare), "a Results section alone is not enough");
        let with_layout = format!("Layout\n\t1\t2\nA\tSPL1\tSPL2\tWell ID\n\n{bare}");
        assert!(sniff(&with_layout));
        let ex = parse(&text_book(with_layout.as_bytes()), "x.txt");
        assert_eq!(ex.blocks[0].channels[0].wavelength_nm, Some(420.0));
        assert!(!ex.blocks[0].channels[0].calculated);
    }

    /// Gen5's Excel export, as `zenodo4449746-gen5-synergy-htx` lays it out: the kinetic table
    /// starts in column B, times are Excel time values (above one day from 24 h on), a depositor
    /// added lines between the title and the table and cells after the wells, and two unnamed
    /// fluorescence reads are told apart by their filter sets. Each worksheet is one export.
    fn excel_sheet(name: &str, plate_time: f64) -> Sheet {
        use crate::sheet::Cell::{Date, Empty, Number, Text};
        let t = |s: &str| Text(s.into());
        let mut rows = vec![
            vec![t("Software Version"), t("3.03.14")],
            vec![t("Plate Number"), t("Plate 1")],
            vec![t("Reader Type:"), t("Synergy HTX")],
            vec![t("Procedure Details")],
            vec![],
            vec![t("Plate Type"), t("96 WELL PLATE")],
            vec![
                t("Start Kinetic"),
                t("Runtime 24:00:00 (HH:MM:SS), Interval 0:10:00, 145 Reads"),
            ],
            vec![t("    Read"), t("Fluorescence Endpoint")],
            vec![Empty, t("Filter Set 1")],
            vec![Empty, t("    Excitation: 485/20,  Emission: 528/20")],
            vec![t("    Read"), t("Fluorescence Endpoint")],
            vec![Empty, t("Filter Set 1")],
            vec![Empty, t("    Excitation: 590/20,  Emission: 635/32")],
            vec![t("End Kinetic")],
            vec![],
        ];
        for (n, spec) in [(1, "485/20,528/20"), (2, "590/20,635/32")] {
            rows.push(vec![t(&format!("Read {n}:{spec}"))]);
            // the depositor's annotation lines
            rows.push(vec![Empty, Empty, Empty, t("replicate 1")]);
            rows.push(vec![t("(GFP)")]);
            rows.push(vec![
                Empty,
                t("Time"),
                t(&format!("T° Read {n}:{spec}")),
                t("A1"),
                t("A2"),
                t("comp. A"),
            ]);
            rows.push(vec![
                Empty,
                Date(plate_time),
                Number(37.0),
                Number(10.0 * f64::from(n)),
                Number(11.0),
                Number(99.0),
            ]);
            rows.push(vec![
                Empty,
                Date(1.005_243_055_555_555_6),
                Number(37.0),
                Number(12.0),
                Number(13.0),
            ]);
            // padding row, then a depositor's cell where the blank line was
            rows.push(vec![Empty, Date(0.0), Number(0.0)]);
            rows.push(vec![Empty, Empty, Empty, Empty, Empty, t("#DIV/0!")]);
        }
        Sheet {
            name: name.into(),
            rows,
        }
    }

    #[test]
    fn excel_kinetic_tables_in_every_sheet() {
        let book = Book {
            sheets: vec![
                excel_sheet("experiment 1", 0.005_243_055_555_555_556),
                Sheet {
                    name: "notes".into(),
                    rows: vec![vec![crate::sheet::Cell::Text("a note".into())]],
                },
                excel_sheet("experiment 2", 0.005_243_055_555_555_556),
            ],
            container: crate::sheet::Container::Workbook { kind: "xlsx" },
        };
        let ex = parse(&book, "x.xlsx");
        assert_eq!(ex.sheets_read, ["experiment 1", "experiment 2"]);
        assert_eq!(ex.blocks.len(), 2);
        let b = &ex.blocks[1];
        assert_eq!(b.name, "Plate 1 (experiment 2)");
        assert_eq!(b.sheet.as_deref(), Some("experiment 2"));
        assert_eq!(b.read_type, Some(ReadType::Kinetic));
        assert!(b.findings.is_empty(), "{:?}", b.findings);
        // two reads, named by the `T°` header where the depositor's line hid the title
        assert_eq!(b.channels.len(), 2);
        assert!(b.channels.iter().all(|c| !c.calculated));
        assert_eq!(b.channels[1].excitation_nm, Some(590.0));
        assert_eq!(b.channels[1].emission_nm, Some(635.0));
        // 2 reads x 2 times x 2 wells; the depositor's `comp. A` column is not a well
        assert_eq!(b.obs.len(), 8);
        let mut times: Vec<f64> = b.obs.iter().filter_map(|o| o.time_s).collect();
        times.sort_by(f64::total_cmp);
        times.dedup();
        assert_eq!(times, [453.0, 86_853.0]);
        assert!(
            ex.header
                .iter()
                .any(|(k, _)| k == "experiment 2 / Software Version")
        );
    }

    /// Worksheets no dialect read are reported: one that is an export of its own as left out
    /// (a warning, and `undecoded` without a scope), any other only named (info).
    #[test]
    fn unread_worksheets_are_reported() {
        use crate::sheet::Cell::Text;
        let tecan = Sheet {
            name: "Sheet1".into(),
            rows: vec![vec![Text("Application: Tecan i-control".into())]],
        };
        let notes = Sheet {
            name: "notes".into(),
            rows: vec![vec![Text("a note".into())]],
        };
        let book = Book {
            sheets: vec![
                tecan,
                excel_sheet("experiment 1", 0.005),
                notes,
                Sheet::default(),
            ],
            container: crate::sheet::Container::Workbook { kind: "xlsx" },
        };
        let kind = crate::sniff_book(&book).unwrap();
        assert_eq!(kind, Kind::TecanIControl);
        let mut ex = crate::parse_kind(kind, &book, "x.xlsx");
        crate::note_unread_sheets(&book, &mut ex);
        assert_eq!(
            ex.unread_sheets,
            [
                ("experiment 1".to_string(), Some(Kind::Gen5)),
                ("notes".to_string(), None)
            ]
        );
        let codes: Vec<_> = ex
            .findings
            .iter()
            .filter(|f| f.code == "worksheet_not_read")
            .map(|f| f.severity)
            .collect();
        assert_eq!(
            codes,
            [
                openreadout_core::model::Severity::Warning,
                openreadout_core::model::Severity::Info
            ]
        );
        let o = crate::assurance::left_out(&ex);
        assert!(
            o.undecoded
                .iter()
                .any(|u| u.structure.contains("experiment 1") && u.scope.is_empty())
        );
    }

    #[test]
    fn not_gen5() {
        assert!(!sniff("Results\n\t1\t2\nA\t1\t2\n"));
        assert!(!sniff("Layout\nsomething else\n"));
    }
}
