//! Results exports of qPCR software (`docs/formats/qpcr.md` § Results exports): the tables the
//! vendor software writes when a run is exported to Excel or text, read where the run file itself
//! is not available or not readable (a Bio-Rad `.pcrd` is encrypted).
//!
//! - Applied Biosystems (StepOne, 7500, QuantStudio, ViiA 7 software): a `Results` sheet (`.xls`,
//!   `.xlsx`) with a key/value header (`Block Type`, `Instrument Type`, `Chemistry`, …) above a
//!   table whose header row starts with `Well`; optional `Amplification Data` (Rn, ΔRn per cycle)
//!   and `Melt Curve Raw Data` sheets.
//! - Bio-Rad CFX Manager / CFX Maestro `Quantification Cq Results` (`.csv`, `.txt`, `.xlsx`): one
//!   header row with `Well`, `Fluor`, `Target`, `Content`, `Sample`, `Cq`, then one line per well
//!   and fluorophore.
//!
//! Every value is read from a named column; a header that names neither layout is refused.
//! Provenance: `docs/provenance/qpcr.md` (2026-10-06).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

use openreadout_core::{Error, Result};

use crate::model::{
    Assay, Curve, Dialect, Dye, Instrument, Melt, QpcrData, Reaction, Run, Sample, Target,
    task_name,
};

pub(crate) const FORMAT_ID: &str = "qpcr-results-export";

/// Most cells a worksheet may expand to (an export with amplification data of a 384-well plate
/// holds about 100 000 rows of six cells).
const MAX_SHEET_CELLS: usize = 8 << 20;

/// One cell of an export.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum XCell {
    Empty,
    Num(f64),
    Text(String),
}

impl XCell {
    pub(crate) fn text(&self) -> String {
        match self {
            XCell::Empty => String::new(),
            XCell::Num(v) => {
                if v.fract() == 0.0 && v.abs() < 1e15 {
                    format!("{v:.0}")
                } else {
                    v.to_string()
                }
            }
            XCell::Text(s) => s.trim().to_string(),
        }
    }

    /// A number, from a numeric cell or a text cell that holds one; `NaN`, `N/A`,
    /// `Undetermined` and empty cells are not numbers.
    pub(crate) fn num(&self) -> Option<f64> {
        match self {
            XCell::Num(v) => Some(*v).filter(|v| v.is_finite()),
            XCell::Text(s) => s.trim().parse::<f64>().ok().filter(|v| v.is_finite()),
            XCell::Empty => None,
        }
    }
}

/// A worksheet (or the single table of a text export) as rows of cells.
#[derive(Debug, Clone)]
pub(crate) struct XSheet {
    pub(crate) name: String,
    pub(crate) rows: Vec<Vec<XCell>>,
}

impl XSheet {
    fn cell(&self, r: usize, c: usize) -> &XCell {
        self.rows
            .get(r)
            .and_then(|row| row.get(c))
            .unwrap_or(&XCell::Empty)
    }

    fn text(&self, r: usize, c: usize) -> String {
        self.cell(r, c).text()
    }
}

/// How the export was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Container {
    Xls,
    Xlsx,
    /// Delimited text: tab or comma.
    Text(char),
}

impl Layout {
    /// Our name for the layout.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Layout::AppliedBiosystems => "Applied Biosystems",
            Layout::BioRadCfx => "Bio-Rad CFX",
        }
    }
}

impl Container {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Container::Xls => "xls workbook",
            Container::Xlsx => "xlsx workbook",
            Container::Text('\t') => "tab-delimited text",
            Container::Text(_) => "comma-delimited text",
        }
    }
}

/// Which software wrote the export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Applied Biosystems `Results` table (StepOne, 7500, QuantStudio, ViiA 7 software).
    AppliedBiosystems,
    /// Bio-Rad CFX `Quantification Cq Results`.
    BioRadCfx,
}

/// Whether the head of a file is a workbook (OLE2 `.xls` or zip `.xlsx`).
pub(crate) fn is_workbook(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04")
        || head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

/// Read an export from its bytes: a workbook (by signature) or delimited text.
pub(crate) fn read_sheets(bytes: Vec<u8>) -> Result<(Container, Vec<XSheet>)> {
    if is_workbook(&bytes) {
        let xlsx = bytes.starts_with(b"PK");
        let sheets =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| workbook(bytes, xlsx)))
                .unwrap_or_else(|_| {
                    Err(Error::corrupt(
                        FORMAT_ID,
                        "the workbook parser failed on this file (damaged workbook)",
                    ))
                })?;
        Ok((
            if xlsx {
                Container::Xlsx
            } else {
                Container::Xls
            },
            sheets,
        ))
    } else {
        let text = decode_text(&bytes);
        let delim = if text.lines().take(50).any(|l| l.contains('\t')) {
            '\t'
        } else {
            ','
        };
        let rows = text.lines().map(|l| split_line(l, delim)).collect();
        Ok((
            Container::Text(delim),
            vec![XSheet {
                name: String::new(),
                rows,
            }],
        ))
    }
}

/// UTF-8 (with or without BOM), UTF-16 with a BOM, else Windows-1252 read byte by byte.
fn decode_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        let u: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let u: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => openreadout_core::bytes::windows1252(b),
    }
}

/// Split one delimited line; double quotes protect delimiters (`""` is a quote).
fn split_line(line: &str, delim: char) -> Vec<XCell> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.trim_end_matches('\r').chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.trim().is_empty() {
            field.clear();
            quoted = true;
        } else if c == delim {
            out.push(text_cell(std::mem::take(&mut field)));
        } else {
            field.push(c);
        }
    }
    out.push(text_cell(field));
    out
}

fn text_cell(s: String) -> XCell {
    if s.trim().is_empty() {
        XCell::Empty
    } else {
        XCell::Text(s)
    }
}

fn workbook(bytes: Vec<u8>, xlsx: bool) -> Result<Vec<XSheet>> {
    use calamine::{Data, Reader, Xls, Xlsx, open_workbook_from_rs};
    let bad = |e: String| Error::corrupt(FORMAT_ID, format!("workbook could not be read: {e}"));
    let conv = |d: &Data| match d {
        Data::Empty => XCell::Empty,
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => text_cell(s.clone()),
        Data::Float(f) => XCell::Num(*f),
        Data::Int(i) => XCell::Num(*i as f64),
        Data::Bool(b) => XCell::Text(if *b { "TRUE" } else { "FALSE" }.into()),
        Data::DateTime(dt) => XCell::Num(dt.as_f64()),
        Data::Error(e) => XCell::Text(format!("#{e:?}")),
    };
    let mut out = Vec::new();
    let c = std::io::Cursor::new(bytes);
    if xlsx {
        let mut wb: Xlsx<_> =
            open_workbook_from_rs(c).map_err(|e: calamine::XlsxError| bad(e.to_string()))?;
        for name in wb.sheet_names() {
            // streamed cell by cell: a damaged sheet's dimension cannot size a dense grid
            let mut rd = wb
                .worksheet_cells_reader(&name)
                .map_err(|e| bad(e.to_string()))?;
            let mut rows: Vec<Vec<XCell>> = Vec::new();
            let mut cells = 0usize;
            while let Some(cell) = rd.next_cell().map_err(|e| bad(e.to_string()))? {
                let v = conv(&Data::from(cell.get_value().clone()));
                if v == XCell::Empty {
                    continue;
                }
                let (r, col) = cell.get_position();
                let (r, col) = (r as usize, col as usize);
                cells = cells.saturating_add(col.saturating_add(1));
                if cells > MAX_SHEET_CELLS || r > MAX_SHEET_CELLS {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("worksheet {name:?} spans more than {MAX_SHEET_CELLS} cells"),
                    ));
                }
                if rows.len() <= r {
                    rows.resize(r + 1, Vec::new());
                }
                let row = &mut rows[r];
                if row.len() <= col {
                    row.resize(col + 1, XCell::Empty);
                }
                row[col] = v;
            }
            out.push(XSheet { name, rows });
        }
    } else {
        let mut wb: Xls<_> =
            open_workbook_from_rs(c).map_err(|e: calamine::XlsError| bad(e.to_string()))?;
        for name in wb.sheet_names() {
            let range = wb.worksheet_range(&name).map_err(|e| bad(e.to_string()))?;
            let (h, w) = range.get_size();
            if h.saturating_mul(w) > MAX_SHEET_CELLS {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("worksheet {name:?} spans more than {MAX_SHEET_CELLS} cells"),
                ));
            }
            let (r0, c0) = range.start().unwrap_or((0, 0));
            let mut rows: Vec<Vec<XCell>> = vec![Vec::new(); r0 as usize];
            for row in range.rows() {
                let mut v: Vec<XCell> = vec![XCell::Empty; c0 as usize];
                v.extend(row.iter().map(conv));
                rows.push(v);
            }
            out.push(XSheet { name, rows });
        }
    }
    Ok(out)
}

/// The Ct column of an Applied Biosystems results table: `CT`, `Ct`, `Cт` (StepOne writes a
/// Cyrillic `т`), `Cq`.
fn is_ct_header(h: &str) -> bool {
    matches!(h, "CT" | "Ct" | "C\u{442}" | "Cq")
}

/// Index of the header row of an Applied Biosystems results table in `s`, if any: a row whose
/// first cell is `Well`, that names `Well Position`, `Sample Name` or `Target Name` and a Ct
/// column, below a header block that names the block or the instrument.
fn ab_header(s: &XSheet) -> Option<usize> {
    let hi = (0..s.rows.len().min(400)).find(|&r| s.text(r, 0) == "Well")?;
    let head: Vec<String> = s.rows[hi].iter().map(XCell::text).collect();
    let names_well = head
        .iter()
        .any(|h| h == "Well Position" || h == "Sample Name" || h == "Target Name");
    let block = (0..hi).any(|r| {
        let k = s.text(r, 0);
        k == "Block Type" || k == "Instrument Type"
    });
    (names_well && block && head.iter().any(|h| is_ct_header(h))).then_some(hi)
}

/// Index of the header row of a Bio-Rad CFX `Quantification Cq Results` table: a row naming
/// `Well`, `Fluor`, `Content` and `Cq`.
fn cfx_header(s: &XSheet) -> Option<usize> {
    (0..s.rows.len().min(50)).find(|&r| {
        let head: Vec<String> = s.rows[r].iter().map(XCell::text).collect();
        ["Well", "Fluor", "Content", "Cq"]
            .iter()
            .all(|k| head.iter().any(|h| h == k))
    })
}

/// Which layout `sheets` hold, if any (the sheet a reader starts from).
pub(crate) fn classify(sheets: &[XSheet]) -> Option<Layout> {
    let results = sheets
        .iter()
        .find(|s| s.name == "Results")
        .into_iter()
        .chain(sheets.iter());
    for s in results {
        if ab_header(s).is_some() {
            return Some(Layout::AppliedBiosystems);
        }
        if cfx_header(s).is_some() {
            return Some(Layout::BioRadCfx);
        }
    }
    None
}

/// `A1`, `A01`, `P24` → zero-based (row, column).
fn parse_well(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let letters: String = s.chars().take_while(char::is_ascii_uppercase).collect();
    let digits = &s[letters.len()..];
    if letters.is_empty() || letters.len() > 2 || digits.is_empty() || digits.len() > 3 {
        return None;
    }
    let col: u32 = digits.parse().ok()?;
    let mut row = 0u32;
    for b in letters.bytes() {
        row = row * 26 + u32::from(b - b'A') + 1;
    }
    (col >= 1).then(|| (row - 1, col - 1))
}

/// Plate geometry (rows, columns) from an Applied Biosystems `Block Type` (`96well`,
/// `384-Well Block`, `Fast 96-Well Block (0.1mL)`, `48well`).
fn block_geometry(block: &str) -> Option<(u32, u32)> {
    if block.contains("384") {
        Some((16, 24))
    } else if block.contains("96") {
        Some((8, 12))
    } else if block.contains("48") {
        Some((6, 8))
    } else {
        None
    }
}

/// The smallest standard plate (96, 384, 1536 wells) that holds every position.
fn smallest_plate(max_row: u32, max_col: u32) -> (u32, u32) {
    if max_row < 8 && max_col < 12 {
        (8, 12)
    } else if max_row < 16 && max_col < 24 {
        (16, 24)
    } else {
        (32, 48)
    }
}

/// Column lookup by header name.
struct Cols(Vec<String>);

impl Cols {
    fn of(s: &XSheet, r: usize) -> Self {
        Cols(s.rows[r].iter().map(XCell::text).collect())
    }
    fn get(&self, names: &[&str]) -> Option<usize> {
        self.0.iter().position(|h| names.contains(&h.as_str()))
    }
}

fn yes(t: &str) -> Option<bool> {
    match t.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" => Some(true),
        "false" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

/// Parse an export into the qPCR model.
pub(crate) fn parse(name: &Path, bytes: Vec<u8>) -> Result<QpcrData> {
    let (container, sheets) = read_sheets(bytes)?;
    let layout = classify(&sheets).ok_or_else(|| Error::UnknownFormat {
        path: name.to_path_buf(),
    })?;
    let mut d = match layout {
        Layout::AppliedBiosystems => parse_ab(&sheets)?,
        Layout::BioRadCfx => parse_cfx(&sheets)?,
    };
    if let Value::Object(m) = &mut d.vendor {
        m.insert("container".into(), json!(container.name()));
        m.insert(
            "sheets".into(),
            json!(sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>()),
        );
    }
    Ok(d)
}

/// Fill samples, targets and dyes from the reactions (in order of first use).
fn collect_definitions(d: &mut QpcrData) {
    let mut samples: Vec<Sample> = Vec::new();
    let mut targets: Vec<Target> = Vec::new();
    let mut dyes: Vec<Dye> = Vec::new();
    for run in &d.runs {
        for rx in &run.reactions {
            if let Some(s) = &rx.sample
                && !samples.iter().any(|x| &x.name == s)
            {
                samples.push(Sample {
                    name: s.clone(),
                    kind: rx.assays.first().and_then(|a| a.task.clone()),
                    ..Sample::default()
                });
            }
            for a in &rx.assays {
                if let Some(t) = &a.target
                    && !targets.iter().any(|x| &x.name == t)
                {
                    targets.push(Target {
                        name: t.clone(),
                        dye: a.dye.clone(),
                        ..Target::default()
                    });
                }
                if let Some(y) = &a.dye
                    && !dyes.iter().any(|x| &x.name == y)
                {
                    dyes.push(Dye {
                        name: y.clone(),
                        chemistry: None,
                    });
                }
            }
        }
    }
    d.samples = samples;
    d.targets = targets;
    d.dyes = dyes;
}

/// Reaction of a plate position, created on first use.
fn reaction(run: &mut Run, row: u32, col: u32) -> &mut Reaction {
    let position = row * run.columns + col;
    let i = if let Some(i) = run.reactions.iter().position(|r| r.position == position) {
        i
    } else {
        run.reactions.push(Reaction {
            position,
            row,
            column: col,
            ..Reaction::default()
        });
        run.reactions.len() - 1
    };
    &mut run.reactions[i]
}

// ------------------------------------------------------------------ Applied Biosystems

/// A row's plate position: `Well Position` (`A1`) or `Well` (`A1`, or a 1-based index on a
/// plate of `cols` columns).
fn ab_position(s: &XSheet, r: usize, cols: &Cols, columns: u32) -> Option<(u32, u32)> {
    if let Some(at) = cols.get(&["Well Position"])
        && let Some(rc) = parse_well(&s.text(r, at))
    {
        return Some(rc);
    }
    if let Some(rc) = parse_well(&s.text(r, 0)) {
        return Some(rc);
    }
    let n = s.cell(r, 0).num()?;
    if n < 1.0 || n.fract() != 0.0 || n > 100_000.0 {
        return None;
    }
    let i = n as u32 - 1;
    Some((i / columns, i % columns))
}

// r = row, c = column lookup, d = the data read, a = one assay: short names keep the column
// lookups readable
#[allow(clippy::many_single_char_names)]
fn parse_ab(sheets: &[XSheet]) -> Result<QpcrData> {
    let results = sheets
        .iter()
        .filter(|s| ab_header(s).is_some())
        .find(|s| s.name == "Results")
        .or_else(|| sheets.iter().find(|s| ab_header(s).is_some()))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no results table"))?;
    let hi = ab_header(results).unwrap_or(0);
    let mut header: Vec<(String, String)> = Vec::new();
    for r in 0..hi {
        let k = results.text(r, 0);
        if !k.is_empty() {
            header.push((k, results.text(r, 1)));
        }
    }
    let get = |k: &str| {
        header
            .iter()
            .find(|(hk, v)| hk == k && !v.is_empty())
            .map(|(_, v)| v.clone())
    };
    let mut d = QpcrData::new(Dialect::AbExport);
    d.instrument = Instrument {
        manufacturer: Some("Applied Biosystems".into()),
        model: get("Instrument Type"),
        serial_number: get("Instrument Serial Number"),
        software: None,
        software_version: None,
        firmware_version: None,
    };
    d.name = get("Experiment Name");
    d.operator = get("Experiment User Name");
    d.chemistry = get("Chemistry");
    d.passive_reference = get("Passive Reference");
    d.experiment_type = get("Experiment Type");
    d.ended_at = get("Experiment Run End Time").and_then(|t| run_end_time(&t));
    let block = get("Block Type").unwrap_or_default();
    let cols = Cols::of(results, hi);
    // rows of the table (to the first line without a well)
    let mut data_rows = Vec::new();
    let mut r = hi + 1;
    while r < results.rows.len() && !results.text(r, 0).is_empty() {
        data_rows.push(r);
        r += 1;
    }
    let (rows_n, cols_n) = if let Some(g) = block_geometry(&block) {
        g
    } else {
        // no block type: the positions the table names
        let mut mr = 0;
        let mut mc = 0;
        for &r in &data_rows {
            if let Some((a, b)) = ab_position(results, r, &cols, 12) {
                mr = mr.max(a);
                mc = mc.max(b);
            }
        }
        smallest_plate(mr, mc)
    };
    let mut run = Run {
        name: d.name.clone().unwrap_or_else(|| "Run 1".into()),
        instrument: d.instrument.model.clone(),
        rows: rows_n,
        columns: cols_n,
        row_label: "ABC".into(),
        column_label: "123".into(),
        ..Run::default()
    };
    let c = |names: &[&str]| cols.get(names);
    let (c_sample, c_target, c_task, c_rep, c_ct) = (
        c(&["Sample Name"]),
        c(&["Target Name", "Detector Name", "Detector"]),
        c(&["Task"]),
        c(&["Reporter"]),
        cols.0.iter().position(|h| is_ct_header(h)),
    );
    let c_quencher = c(&["Quencher"]);
    let c_omit = c(&["Omit"]);
    let mut skipped = 0usize;
    let mut quenchers: BTreeMap<String, String> = BTreeMap::new();
    let flag_cols: Vec<(usize, String)> = cols
        .0
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            h.len() >= 3
                && h.chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
                && h.as_str() != "RQ"
                && h.as_str() != "CT"
        })
        .map(|(i, h)| (i, h.clone()))
        .collect();
    for &r in &data_rows {
        let Some((row, col)) = ab_position(results, r, &cols, cols_n) else {
            skipped += 1;
            continue;
        };
        if row >= rows_n || col >= cols_n {
            skipped += 1;
            continue;
        }
        let txt = |ci: Option<usize>| ci.map(|i| results.text(r, i)).filter(|v| !v.is_empty());
        let numc = |ci: Option<usize>| ci.and_then(|i| results.cell(r, i).num());
        let target = txt(c_target);
        let ct_text = txt(c_ct);
        let sample = txt(c_sample);
        if target.is_none() && ct_text.is_none() && sample.is_none() {
            continue; // an unused well
        }
        let mut a = Assay {
            target: target.clone(),
            dye: txt(c_rep),
            task: txt(c_task).map(|t| task_name(&t)),
            ..Assay::default()
        };
        match ct_text.as_deref() {
            Some(t) if t.eq_ignore_ascii_case("undetermined") => a.cq_undetermined = true,
            Some(_) => a.cq = numc(c_ct),
            None => {}
        }
        a.cq_mean = numc(c(&["Ct Mean", "C\u{442} Mean", "Cq Mean"]));
        a.cq_sd = numc(c(&["Ct SD", "C\u{442} SD", "Cq SD"]));
        let q = numc(c(&["Quantity"]));
        if a.task.as_deref() == Some("standard") {
            a.quantity = q;
        } else {
            a.calculated_quantity = q;
        }
        a.threshold = numc(c(&["Ct Threshold"]));
        a.threshold_used = a.threshold.is_some();
        a.auto_threshold = txt(c(&["Automatic Ct Threshold"])).and_then(|t| yes(&t));
        a.auto_baseline = txt(c(&["Automatic Baseline"])).and_then(|t| yes(&t));
        a.baseline_start = numc(c(&["Baseline Start"])).map(|v| v as u32);
        a.baseline_end = numc(c(&["Baseline End"])).map(|v| v as u32);
        a.amp_status = txt(c(&["Amp Status"])).and_then(|t| match t.as_str() {
            "Amp" => Some("amplified".into()),
            "No Amp" => Some("not amplified".into()),
            "Inconclusive" => Some("inconclusive".into()),
            _ => None,
        });
        a.cq_confidence = numc(c(&["Cq Conf"]));
        for k in ["Tm1", "Tm2", "Tm3", "Tm4"] {
            if let Some(v) = numc(c(&[k])) {
                a.tm.push(v);
            }
        }
        a.vendor_rq = numc(c(&["RQ"]));
        a.vendor_delta_cq = numc(c(&["Delta Ct", "\u{394}C\u{442}"]));
        a.vendor_delta_delta_cq = numc(c(&["Delta Delta Ct", "\u{394}\u{394}C\u{442}"]));
        a.note = txt(c(&["Comments"]));
        if txt(c_omit).and_then(|t| yes(&t)) == Some(true) {
            a.excluded = Some("omitted in the vendor software".into());
        }
        for (i, name) in &flag_cols {
            if results.text(r, *i) == "Y" {
                a.flags.push(name.to_ascii_lowercase());
            }
        }
        if let (Some(t), Some(qn)) = (&target, txt(c_quencher)) {
            quenchers.insert(t.clone(), qn);
        }
        let rx = reaction(&mut run, row, col);
        if rx.sample.is_none() {
            rx.sample = sample;
        }
        rx.assays.push(a);
    }
    // curves from the other sheets
    let mut notes = Vec::new();
    if let Some(s) = sheets.iter().find(|s| s.name == "Amplification Data") {
        attach_amplification(s, &mut run, &mut notes);
    }
    if let Some(s) = sheets.iter().find(|s| s.name == "Melt Curve Raw Data") {
        attach_melt(s, &mut run, &mut notes);
    }
    run.reactions.sort_by_key(|r| r.position);
    if skipped > 0 {
        notes.push(format!(
            "{skipped} result lines give no plate position and were skipped"
        ));
    }
    let undetermined = run
        .reactions
        .iter()
        .flat_map(|r| &r.assays)
        .filter(|a| a.cq_undetermined)
        .count();
    if undetermined > 0 {
        notes.push(format!(
            "{undetermined} results are Undetermined in the export (no Cq)"
        ));
    }
    let mut vendor = Map::new();
    vendor.insert("layout".into(), json!("applied-biosystems results export"));
    vendor.insert(
        "header".into(),
        Value::Object(header.into_iter().map(|(k, v)| (k, json!(v))).collect()),
    );
    vendor.insert("columns".into(), json!(cols.0));
    d.vendor = Value::Object(vendor);
    d.notes = notes;
    d.runs.push(run);
    collect_definitions(&mut d);
    for t in &mut d.targets {
        t.quencher = quenchers.get(&t.name).cloned().filter(|q| q != "None");
    }
    Ok(d)
}

/// `2023-04-21 12:14:09 PM CEST` → `2023-04-21T12:14:09` (the zone is not converted; `Not
/// Started` and other text → none).
fn run_end_time(t: &str) -> Option<String> {
    let mut parts = t.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let ampm = parts.next();
    let dp: Vec<&str> = date.split('-').collect();
    if dp.len() != 3
        || dp[0].len() != 4
        || !dp.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let tp: Vec<u32> = time
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if tp.len() != 3 {
        return None;
    }
    let mut h = tp[0];
    match ampm {
        // 24-hour clocks also carry AM/PM in some exports (`15:47:40 PM`)
        Some("PM") if h < 12 => h += 12,
        Some("AM") if h == 12 => h = 0,
        _ => {}
    }
    if h > 23 || tp[1] > 59 || tp[2] > 60 {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{h:02}:{:02}:{:02}",
        dp[0], dp[1], dp[2], tp[1], tp[2]
    ))
}

/// `Amplification Data`: `Well`, [`Well Position`], `Cycle`, `Target Name`, `Rn`, `Delta Rn`.
fn attach_amplification(s: &XSheet, run: &mut Run, notes: &mut Vec<String>) {
    let Some(hi) = (0..s.rows.len().min(400)).find(|&r| s.text(r, 0) == "Well") else {
        return;
    };
    let cols = Cols::of(s, hi);
    let (Some(cc), Some(crn)) = (cols.get(&["Cycle"]), cols.get(&["Rn"])) else {
        notes.push("Amplification Data sheet without Cycle and Rn columns: not read".into());
        return;
    };
    let (ct, cd) = (cols.get(&["Target Name"]), cols.get(&["Delta Rn"]));
    let mut curves: BTreeMap<(u32, u32, String), Curve> = BTreeMap::new();
    for r in hi + 1..s.rows.len() {
        if s.text(r, 0).is_empty() {
            break;
        }
        let Some(pos) = ab_position(s, r, &cols, run.columns) else {
            continue;
        };
        let target = ct.map(|i| s.text(r, i)).unwrap_or_default();
        let (Some(cycle), Some(rn)) = (s.cell(r, cc).num(), s.cell(r, crn).num()) else {
            continue;
        };
        let e = curves
            .entry((pos.0, pos.1, target))
            .or_insert_with(|| Curve {
                quantity: "Rn",
                corrected: cd.map(|_| Vec::new()),
                ..Curve::default()
            });
        e.cycles.push(cycle);
        e.fluorescence.push(rn);
        if let (Some(i), Some(v)) = (cd, e.corrected.as_mut()) {
            v.push(s.cell(r, i).num().unwrap_or(f64::NAN));
        }
    }
    let mut unmatched = 0usize;
    for ((row, col, target), curve) in curves {
        let position = row * run.columns + col;
        let Some(rx) = run.reactions.iter_mut().find(|x| x.position == position) else {
            unmatched += 1;
            continue;
        };
        // the assay of that target, or the well's only assay when the sheet names none
        let n = rx.assays.len();
        let a = rx
            .assays
            .iter_mut()
            .find(|a| a.target.as_deref() == Some(target.as_str()));
        match a {
            Some(a) => a.amplification = Some(curve),
            None if target.is_empty() && n == 1 => rx.assays[0].amplification = Some(curve),
            None => unmatched += 1,
        }
    }
    if unmatched > 0 {
        notes.push(format!(
            "{unmatched} amplification curves of the export match no result line and were left out"
        ));
    }
}

/// `Melt Curve Raw Data`: `Well`, `Well Position`, `Reading`, `Temperature`, `Fluorescence`,
/// `Derivative`, [`Target Name`].
fn attach_melt(s: &XSheet, run: &mut Run, notes: &mut Vec<String>) {
    let Some(hi) = (0..s.rows.len().min(400)).find(|&r| s.text(r, 0) == "Well") else {
        return;
    };
    let cols = Cols::of(s, hi);
    let (Some(ctemp), Some(cf)) = (cols.get(&["Temperature"]), cols.get(&["Fluorescence"])) else {
        notes.push(
            "Melt Curve Raw Data sheet without Temperature and Fluorescence: not read".into(),
        );
        return;
    };
    let (ct, cd) = (cols.get(&["Target Name"]), cols.get(&["Derivative"]));
    let mut melts: BTreeMap<(u32, u32, String), (Melt, Vec<f64>)> = BTreeMap::new();
    for r in hi + 1..s.rows.len() {
        if s.text(r, 0).is_empty() {
            break;
        }
        let Some(pos) = ab_position(s, r, &cols, run.columns) else {
            continue;
        };
        let (Some(t), Some(f)) = (s.cell(r, ctemp).num(), s.cell(r, cf).num()) else {
            continue;
        };
        let target = ct.map(|i| s.text(r, i)).unwrap_or_default();
        let e = melts.entry((pos.0, pos.1, target)).or_default();
        e.0.temperature.push(t);
        e.0.fluorescence.push(f);
        if let Some(i) = cd {
            e.1.push(s.cell(r, i).num().unwrap_or(f64::NAN));
        }
    }
    let mut unmatched = 0usize;
    for ((row, col, target), (mut melt, deriv)) in melts {
        if deriv.len() == melt.temperature.len() && !deriv.is_empty() {
            melt.derivative = Some((melt.temperature.clone(), deriv));
        }
        let position = row * run.columns + col;
        let Some(rx) = run.reactions.iter_mut().find(|x| x.position == position) else {
            unmatched += 1;
            continue;
        };
        let n = rx.assays.len();
        if let Some(a) = rx
            .assays
            .iter_mut()
            .find(|a| a.target.as_deref() == Some(target.as_str()))
        {
            a.melt = Some(melt);
        } else if target.is_empty() && n == 1 {
            rx.assays[0].melt = Some(melt);
        } else {
            unmatched += 1;
        }
    }
    if unmatched > 0 {
        notes.push(format!(
            "{unmatched} melt curves of the export match no single result line (a well with several targets and a sheet that names none) and were left out"
        ));
    }
}

// ------------------------------------------------------------------ Bio-Rad CFX

/// CFX `Content` → our task vocabulary (`Unkn-01` is a replicate group of `Unkn`). Other text
/// is not a task.
fn cfx_task(content: &str) -> Option<String> {
    let base = content
        .rsplit_once('-')
        .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map_or(content, |(b, _)| b)
        .trim();
    Some(
        match base {
            "Unkn" => "unknown",
            "Std" => "standard",
            "NTC" => "ntc",
            "NRT" => "nrt",
            "Pos Ctrl" | "Pos" => "positive",
            "Neg Ctrl" | "Neg" => "negative",
            _ => return None,
        }
        .into(),
    )
}

// s = sheet, r = row, c = column lookup, d = the data read, a = one assay
#[allow(clippy::many_single_char_names)]
fn parse_cfx(sheets: &[XSheet]) -> Result<QpcrData> {
    let s = sheets
        .iter()
        .find(|s| cfx_header(s).is_some())
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no Cq results table"))?;
    let hi = cfx_header(s).unwrap_or(0);
    let cols = Cols::of(s, hi);
    let c = |n: &[&str]| cols.get(n);
    let (cw, cf, ccontent, ccq) = (
        c(&["Well"]).unwrap_or(0),
        c(&["Fluor"]),
        c(&["Content"]),
        c(&["Cq"]),
    );
    let (ctarget, csample) = (c(&["Target"]), c(&["Sample"]));
    let mut lines = Vec::new();
    let mut r = hi + 1;
    while r < s.rows.len() && !s.text(r, cw).is_empty() {
        lines.push(r);
        r += 1;
    }
    let mut max = (0, 0);
    for &r in &lines {
        if let Some((a, b)) = parse_well(&s.text(r, cw)) {
            max = (max.0.max(a), max.1.max(b));
        }
    }
    let (rows_n, cols_n) = smallest_plate(max.0, max.1);
    let mut run = Run {
        name: "Run 1".into(),
        rows: rows_n,
        columns: cols_n,
        row_label: "ABC".into(),
        column_label: "123".into(),
        ..Run::default()
    };
    let mut skipped = 0usize;
    let mut other_content: Vec<String> = Vec::new();
    for &r in &lines {
        let Some((row, col)) = parse_well(&s.text(r, cw)) else {
            skipped += 1;
            continue;
        };
        let txt = |ci: Option<usize>| ci.map(|i| s.text(r, i)).filter(|v| !v.is_empty());
        let numc = |ci: Option<usize>| ci.and_then(|i| s.cell(r, i).num());
        let content = txt(ccontent).unwrap_or_default();
        let task = cfx_task(&content);
        if task.is_none() && !content.is_empty() && !other_content.contains(&content) {
            other_content.push(content.clone());
        }
        let mut a = Assay {
            target: txt(ctarget),
            dye: txt(cf),
            task,
            ..Assay::default()
        };
        // CFX writes `NaN` (CFX Maestro) or `N/A` (CFX Manager) where the software found no Cq
        match txt(ccq).as_deref() {
            Some(t) if t.eq_ignore_ascii_case("nan") || t.eq_ignore_ascii_case("n/a") => {
                a.cq_undetermined = true;
            }
            Some(_) => a.cq = numc(ccq),
            None => {}
        }
        a.cq_mean = numc(c(&["Cq Mean"]));
        a.cq_sd = numc(c(&["Cq Std. Dev"]));
        let sq = numc(c(&["Starting Quantity (SQ)", "SQ"]));
        if a.task.as_deref() == Some("standard") {
            a.quantity = sq;
        } else {
            a.calculated_quantity = sq;
        }
        a.note = txt(c(&["Well Note"]));
        let rx = reaction(&mut run, row, col);
        if rx.sample.is_none() {
            rx.sample = txt(csample);
        }
        rx.assays.push(a);
    }
    run.reactions.sort_by_key(|r| r.position);
    let mut d = QpcrData::new(Dialect::CfxExport);
    d.instrument.manufacturer = Some("Bio-Rad".into());
    let mut notes = Vec::new();
    if skipped > 0 {
        notes.push(format!(
            "{skipped} result lines give no plate position and were skipped"
        ));
    }
    if !other_content.is_empty() {
        notes.push(format!(
            "Content values that name no task were left as no task: {}",
            other_content.join(", ")
        ));
    }
    let undetermined = run
        .reactions
        .iter()
        .flat_map(|r| &r.assays)
        .filter(|a| a.cq_undetermined)
        .count();
    if undetermined > 0 {
        notes.push(format!(
            "{undetermined} results have no Cq in the export (NaN or N/A)"
        ));
    }
    notes.push("the export holds no plate size: the plate is the smallest standard plate that holds every well".into());
    let mut vendor = Map::new();
    vendor.insert(
        "layout".into(),
        json!("bio-rad cfx quantification cq results"),
    );
    vendor.insert("columns".into(), json!(cols.0));
    d.vendor = Value::Object(vendor);
    d.notes = notes;
    d.runs.push(run);
    collect_definitions(&mut d);
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Vec<u8> {
        t.as_bytes().to_vec()
    }

    #[test]
    fn wells_and_times() {
        assert_eq!(parse_well("A1"), Some((0, 0)));
        assert_eq!(parse_well("A01"), Some((0, 0)));
        assert_eq!(parse_well("P24"), Some((15, 23)));
        assert_eq!(parse_well("AF48"), Some((31, 47)));
        assert_eq!(parse_well("1"), None);
        assert_eq!(parse_well("A0"), None);
        assert_eq!(
            run_end_time("2023-04-21 12:14:09 PM CEST").as_deref(),
            Some("2023-04-21T12:14:09")
        );
        assert_eq!(
            run_end_time("2017-11-21 15:47:40 PM CET").as_deref(),
            Some("2017-11-21T15:47:40")
        );
        assert_eq!(run_end_time("Not Started"), None);
        assert_eq!(block_geometry("384-Well Block"), Some((16, 24)));
        assert_eq!(block_geometry("Fast 96-Well Block (0.1mL)"), Some((8, 12)));
    }

    #[test]
    fn applied_biosystems_text_results() {
        let t = "Block Type\t96well\nChemistry\tSYBR_GREEN\nInstrument Type\tsteponeplus\n\nWell\tSample Name\tTarget Name\tTask\tReporter\tQuencher\tC\u{442}\tC\u{442} Mean\tCt Threshold\tAutomatic Ct Threshold\tTm1\tHIGHSD\nA1\tS1\tGAPDH\tUNKNOWN\tSYBR\tNone\t17.5\t17.6\t0.2\ttrue\t82.1\tN\nA2\tNTC\tGAPDH\tNTC\tSYBR\tNone\tUndetermined\t\t0.2\ttrue\t\tY\nA3\t\t\t\t\t\t\t\t\t\t\t\n\nRQ Min/Max Confidence Level\t95.0\n";
        let d = parse(Path::new("x.txt"), text(t)).unwrap();
        assert_eq!(d.dialect, Dialect::AbExport);
        assert_eq!(d.instrument.model.as_deref(), Some("steponeplus"));
        let run = &d.runs[0];
        assert_eq!((run.rows, run.columns), (8, 12));
        assert_eq!(run.reactions.len(), 2);
        let a = &run.reactions[0].assays[0];
        assert_eq!(a.cq, Some(17.5));
        assert_eq!(a.tm, vec![82.1]);
        assert_eq!(a.threshold, Some(0.2));
        assert_eq!(a.auto_threshold, Some(true));
        let b = &run.reactions[1].assays[0];
        assert!(b.cq_undetermined && b.cq.is_none());
        assert_eq!(b.task.as_deref(), Some("ntc"));
        assert_eq!(b.flags, vec!["highsd".to_string()]);
        assert_eq!(d.targets.len(), 1);
    }

    #[test]
    fn cfx_csv_results() {
        let t = ",Well,Fluor,Target,Content,Sample,Cq,Cq Mean,Cq Std. Dev,Starting Quantity (SQ)\n,A01,SYBR,GLP1,Unkn-01,S1,25.5,25.6,0.1,NaN\n,A02,SYBR,GLP1,NTC,,NaN,0.00,0.000,NaN\n,P24,FAM,,Std-02,,30.0,,,100\n";
        let d = parse(Path::new("x.csv"), text(t)).unwrap();
        assert_eq!(d.dialect, Dialect::CfxExport);
        let run = &d.runs[0];
        assert_eq!((run.rows, run.columns), (16, 24));
        let a = &run.reactions[0].assays[0];
        assert_eq!((a.cq, a.task.as_deref()), (Some(25.5), Some("unknown")));
        assert!(run.reactions[1].assays[0].cq_undetermined);
        let s = &run.reactions[2].assays[0];
        assert_eq!(
            (s.task.as_deref(), s.quantity),
            (Some("standard"), Some(100.0))
        );
    }

    #[test]
    fn other_tables_are_refused() {
        assert!(parse(Path::new("x.csv"), text("a,b\n1,2\n")).is_err());
        // a Well header without the vendor header block is not an Applied Biosystems export
        assert!(
            parse(
                Path::new("x.txt"),
                text("Well\tSample Name\tCT\nA1\tS\t20\n")
            )
            .is_err()
        );
        assert!(parse(Path::new("x.xlsx"), b"PK\x03\x04garbage".to_vec()).is_err());
    }
}
