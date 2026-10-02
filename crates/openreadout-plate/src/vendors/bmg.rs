//! BMG LABTECH exports: MARS data-analysis CSV/TXT (PHERAstar, CLARIOstar, FLUOstar, POLARstar,
//! SPECTROstar, NEPHELOstar) and SMART Control XLSX (numbered sections, one sheet per view).
//!
//! Both start with `User:`, `Path:`, `Test name:`/`Test Name:`, `Date:`, `Time:`, `ID1:`…`ID3:`
//! pairs (several per line, or one per line) and a line naming the read mode (`Absorbance`,
//! `Fluorescence (FI)`, `Luminescence`, `Absorbance spectrum`, `Fluorescence (FI) spectrum`).
//! MARS writes the values in one of two views:
//! - **plate view**: titled plate matrices, `Raw Data (<filters>)` for measured values, other
//!   titles (`Blank corrected based on …`, `Layout`, `Standard Concentrations …`) for derived
//!   values and layouts;
//! - **table view**: one line per well, `Well` (or `Well Row` + `Well Col`) and `Content`
//!   columns, then one column per value under a column title (`Raw Data (Abs Spectrum)`,
//!   `Average over replicates based on Raw Data (Ex Spectrum)`, `Raw Data (638-12/675-12 1)`).
//!   In a spectral scan a second header line `Wavelength [nm]` gives each column's wavelength.

use std::collections::BTreeMap;

use openreadout_core::model::Finding;
use serde_json::json;

use super::{
    calculated, colon_pairs, filter_pair, key_value, parenthesized, push_grid, titled_grids,
};
use crate::grid::{parse_row_label, parse_well, well_name};
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType, first_number, nonempty};
use crate::sheet::{Book, Sheet};

pub(crate) fn sniff(text: &str) -> bool {
    let head: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(8)
        .map(|l| l.trim_start_matches('\u{feff}'))
        .collect();
    let first = head.first().copied().unwrap_or("");
    // `User: …` first; the path, run number or test id on the same line or the lines after
    // (one `Key: value` pair per line in some exports); a test name within the first lines.
    first.starts_with("User:")
        && head.iter().take(4).any(|l| {
            l.contains("Path:") || l.contains("Test run no.:") || l.starts_with("Test ID:")
        })
        && head
            .iter()
            .any(|l| l.contains("Test name:") || l.contains("Test Name:") || l.contains("Test ID:"))
}

/// SMART Control workbooks: a `Protocol Information` sheet or a `…SMART Control…` path.
pub(crate) fn is_smart_control(book: &Book) -> bool {
    book.sheets.iter().any(|s| s.name == "Protocol Information")
        && book
            .sheets
            .iter()
            .any(|s| (0..s.rows.len().min(20)).any(|r| s.text(r, 0).starts_with("Test ID:")))
}

/// MARS written into a workbook (same lines, one pair per cell).
pub(crate) fn is_mars_book(book: &Book) -> bool {
    book.sheets.iter().any(|s| {
        (0..s.rows.len().min(4)).any(|r| s.text(r, 0).starts_with("User:"))
            && (0..s.rows.len().min(8)).any(|r| s.row_line(r).contains("Test name:"))
    })
}

/// The line naming the read mode: its first cell (`Absorbance spectrum`, `Fluorescence (FI)`),
/// with the rest of the line when short (`Fluorescence (FI), multichromatic`).
fn read_mode_line(sheet: &Sheet, upto: usize) -> Option<(Mode, String)> {
    (0..upto.min(sheet.rows.len())).find_map(|r| {
        let first = sheet.text(r, 0);
        if first.contains(':') || first.len() > 60 {
            return None;
        }
        let m = Mode::from_text(&first);
        if m == Mode::Unknown {
            return None;
        }
        let line = sheet.row_line(r);
        Some((m, if line.len() <= 60 { line } else { first }))
    })
}

fn instrument_from_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split(['\\', '/']).collect();
    let i = parts.iter().position(|p| p.eq_ignore_ascii_case("BMG"))?;
    parts
        .get(i + 1)
        .filter(|p| {
            !p.eq_ignore_ascii_case("MARS")
                && !p.eq_ignore_ascii_case("SMART Control")
                && !p.is_empty()
        })
        .map(|p| (*p).to_string())
}

/// What a spectral scan varies, from the raw-data label (`Abs Spectrum`, `Ex Spectrum`,
/// `Em Spectrum`).
fn scanned(label: &str) -> Option<&'static str> {
    let l = label.to_ascii_lowercase();
    if !l.contains("spectrum") {
        return None;
    }
    if l.contains("abs spectrum") {
        Some("absorbance")
    } else if l.contains("ex spectrum") {
        Some("excitation")
    } else if l.contains("em spectrum") {
        Some("emission")
    } else {
        None
    }
}

fn raw_channel(title: &str, mode: Mode) -> Channel {
    let label = title
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .trim();
    let mut c = Channel::new(label, mode);
    let inner = parenthesized(label).unwrap_or("");
    if let Some(s) = scanned(inner) {
        // A spectral scan: the wavelength of every value is its column's; the label names the
        // scanned light path only.
        c.settings.insert("scanned_wavelength".into(), json!(s));
        c.unit = Some(
            if mode == Mode::Absorbance {
                "OD"
            } else {
                "RFU"
            }
            .into(),
        );
        return c;
    }
    let (ex, em, exb, emb) = filter_pair(inner);
    match mode {
        Mode::Absorbance => {
            c.wavelength_nm = first_number(inner);
            c.unit = Some("OD".into());
        }
        Mode::Luminescence => {
            c.emission_nm = em.filter(|_| !inner.eq_ignore_ascii_case("No filter"));
            c.unit = Some("RLU".into());
        }
        _ => {
            c.excitation_nm = ex;
            c.emission_nm = em;
            c.unit = Some("RFU".into());
            if let Some(b) = exb {
                c.settings
                    .insert("excitation_bandwidth_nm".into(), json!(b));
            }
            if let Some(b) = emb {
                c.settings.insert("emission_bandwidth_nm".into(), json!(b));
            }
        }
    }
    c
}

/// The channel of a table-view column title: `Raw Data (…)` is measured; anything else
/// (`Blank corrected based on Raw Data (…)`, `Average over replicates based on Raw Data (…)`)
/// was calculated by MARS, with the optics of the raw data it names.
fn table_channel(title: &str, mode: Mode) -> Channel {
    let bare = title
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .trim();
    if bare.starts_with("Raw Data") {
        return raw_channel(bare, mode);
    }
    let like = bare.find("Raw Data").map(|i| raw_channel(&bare[i..], mode));
    let mut c = calculated(
        if bare.is_empty() {
            "Unlabelled column"
        } else {
            bare
        },
        like.as_ref(),
    );
    if let Some(l) = &like {
        c.unit.clone_from(&l.unit);
        if let Some(s) = l.settings.get("scanned_wavelength") {
            c.settings.insert("scanned_wavelength".into(), s.clone());
        }
    }
    c
}

/// Where the well of a table-view line is written.
#[derive(Debug, Clone, Copy)]
enum WellColumns {
    /// `A01` in one column.
    Name(usize),
    /// `A` and `1` in two columns (`Well Row`, `Well Col`).
    RowCol(usize, usize),
}

/// A MARS table view located in a sheet.
#[derive(Debug)]
struct TableView {
    /// Line of the column titles.
    header: usize,
    well: WellColumns,
    content: Option<usize>,
    /// First value column.
    first_value: usize,
    /// Per value column (from `first_value`): its wavelength, when a `Wavelength [nm]` line
    /// follows the titles.
    wavelengths: Option<Vec<Option<f64>>>,
    /// A second header line of another kind (`Time [s]`, cycles): not decoded.
    other_axis: Option<String>,
    /// First well line.
    data: usize,
}

fn find_table(sheet: &Sheet, upto: usize) -> Option<TableView> {
    for r in 0..upto.min(sheet.rows.len()) {
        let c0 = sheet.text(r, 0);
        let (well, next) = if c0.eq_ignore_ascii_case("Well") {
            (WellColumns::Name(0), 1)
        } else if c0.eq_ignore_ascii_case("Well Row")
            && sheet.text(r, 1).eq_ignore_ascii_case("Well Col")
        {
            (WellColumns::RowCol(0, 1), 2)
        } else {
            continue;
        };
        let content = sheet
            .text(r, next)
            .eq_ignore_ascii_case("Content")
            .then_some(next);
        let first_value = content.map_or(next, |c| c + 1);
        if sheet.row_len(r) <= first_value {
            continue;
        }
        // An axis line: its label sits in the column before the values.
        let axis_label = sheet.text(r + 1, first_value - 1);
        let well_at = |row: usize| match well {
            WellColumns::Name(c) => parse_well(&sheet.text(row, c)).is_some(),
            WellColumns::RowCol(a, _) => parse_row_label(&sheet.text(row, a), false).is_some(),
        };
        let (wavelengths, other_axis, data) = if well_at(r + 1) {
            (None, None, r + 1)
        } else if axis_label.to_ascii_lowercase().starts_with("wavelength") {
            let width = sheet.row_len(r + 1).max(sheet.row_len(r));
            let wl = (first_value..width)
                .map(|c| sheet.cell(r + 1, c).number())
                .collect();
            (Some(wl), None, r + 2)
        } else {
            (None, Some(axis_label), r + 2)
        };
        return Some(TableView {
            header: r,
            well,
            content,
            first_value,
            wavelengths,
            other_axis,
            data,
        });
    }
    None
}

/// Values of a table view into `b`; the `Content` column becomes the layout.
fn push_table(
    sheet: &Sheet,
    t: &TableView,
    mode: Mode,
    b: &mut Block,
    layout: &mut BTreeMap<String, BTreeMap<String, String>>,
) {
    let width = (t.header..t.data.max(t.header + 1))
        .map(|r| sheet.row_len(r))
        .max()
        .unwrap_or(0);
    // Column titles; a blank title continues the one before it.
    let mut titles: Vec<String> = Vec::new();
    for c in t.first_value..width {
        let s = sheet.text(t.header, c);
        let s = if s.is_empty() {
            titles.last().cloned().unwrap_or_default()
        } else {
            s
        };
        titles.push(s);
    }
    let mut channel_of: BTreeMap<String, u32> = BTreeMap::new();
    for title in &titles {
        if !channel_of.contains_key(title) {
            let i = b.channel(table_channel(title, mode));
            channel_of.insert(title.clone(), i);
        }
    }
    let mut r = t.data;
    while r < sheet.rows.len() {
        let well = match t.well {
            WellColumns::Name(c) => parse_well(&sheet.text(r, c)),
            WellColumns::RowCol(a, bcol) => parse_row_label(&sheet.text(r, a), false).zip(
                sheet
                    .cell(r, bcol)
                    .number()
                    .filter(|n| n.fract() == 0.0 && *n >= 1.0 && *n <= 999.0)
                    .map(|n| n as u32 - 1),
            ),
        };
        let Some((row, col)) = well else {
            break;
        };
        if let Some(c) = t.content {
            let content = sheet.text(r, c);
            if !content.is_empty() {
                layout
                    .entry("Content".into())
                    .or_default()
                    .insert(well_name(row, col), content);
            }
        }
        for (i, title) in titles.iter().enumerate() {
            let cell = sheet.cell(r, t.first_value + i);
            let wl = t
                .wavelengths
                .as_ref()
                .and_then(|w| w.get(i).copied().flatten());
            b.push_cell(row, col, channel_of[title], None, wl, cell);
        }
        r += 1;
    }
    if b.line.is_none() {
        b.line = Some(t.header + 1);
    }
}

pub(crate) fn parse(book: &Book, smart_control: bool) -> Export {
    let kind = if smart_control {
        Kind::BmgSmartControl
    } else {
        Kind::BmgMars
    };
    let mut ex = Export::new(kind, book.container.clone());
    let data_sheet = book
        .sheets
        .iter()
        .find(|s| {
            s.name.contains("Microplate")
                || s.name.contains("End point") && !s.name.starts_with("Table")
        })
        .or_else(|| {
            book.sheets
                .iter()
                .find(|s| !crate::grid::find_grids(s).is_empty())
        })
        .unwrap_or(&book.sheets[0]);
    let grids = titled_grids(data_sheet);
    let table = if grids.is_empty() {
        find_table(data_sheet, 200)
    } else {
        None
    };
    let first_data = grids
        .first()
        .map(|g| g.grid.header_row)
        .or_else(|| table.as_ref().map(|t| t.header))
        .unwrap_or(data_sheet.rows.len());
    for r in 0..first_data.min(40) {
        for (k, v) in colon_pairs(data_sheet, r) {
            ex.put(k, v);
        }
    }
    let (mode, mode_text) =
        read_mode_line(data_sheet, first_data).unwrap_or((Mode::Unknown, String::new()));
    if !mode_text.is_empty() {
        ex.put("Read mode", mode_text.clone());
    }
    // Protocol sheet (SMART Control): `Key:` | value pairs.
    let mut protocol = BTreeMap::new();
    for s in book
        .sheets
        .iter()
        .filter(|s| s.name == "Protocol Information")
    {
        for r in 0..s.rows.len() {
            if let Some((k, v)) = key_value(s, r)
                && !k.starts_with("User")
            {
                protocol.insert(k, v);
            }
        }
    }
    if !protocol.is_empty() {
        ex.sections.insert("protocol".into(), json!(protocol));
    }
    let path = ex.get("Path").map(str::to_string);
    ex.model = path.as_deref().and_then(instrument_from_path);
    ex.protocol = ex
        .get("Test name")
        .or_else(|| ex.get("Test Name"))
        .map(str::to_string);
    ex.operator = ex.get("User").map(str::to_string);
    if let Some(d) = ex.get("Date").map(str::to_string) {
        let t = ex.get("Time").map(str::to_string);
        ex.set_acquired(&d, t.as_deref());
    }
    let id1 = ex.get("ID1").map(str::to_string);
    let plate = id1.clone().unwrap_or_else(|| "Plate 1".into());
    let mut b = Block::new(plate.clone(), plate);
    b.barcode = id1;
    b.read_type = Some(ReadType::Endpoint);
    b.started_at.clone_from(&ex.acquired_at);
    b.sheet = (!data_sheet.name.is_empty()).then(|| data_sheet.name.clone());
    for (k, key) in [
        ("ID2", "id2"),
        ("ID3", "id3"),
        ("Test run no.", "test_run"),
        ("Test ID", "test_id"),
    ] {
        if let Some(v) = ex.get(k).and_then(nonempty) {
            b.extra.insert(key.into(), json!(v));
        }
    }
    if let Some(p) = protocol.get("Microplate name") {
        b.plate_type = Some(p.clone());
        b.declared_wells = first_number(p)
            .map(|n| n as u32)
            .filter(|n| crate::grid::dims_for_wells(*n).is_some());
    }
    let mut layout: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    if let Some(t) = &table {
        b.extra.insert("export_view".into(), json!("table"));
        if let Some(axis) = &t.other_axis {
            // Right or refuse: only the wavelength axis of spectral scans is validated.
            b.findings.push(Finding::warning(
                "table_axis_not_decoded",
                format!(
                    "{}: the table's second header line ({axis:?}) is not a wavelength axis; its values are not read",
                    b.name
                ),
            ));
        } else {
            if t.wavelengths.is_some() {
                b.read_type = Some(ReadType::Spectrum);
            }
            push_table(data_sheet, t, mode, &mut b, &mut layout);
        }
    }
    let mut last_raw: Option<Channel> = None;
    for t in &grids {
        let title = t
            .find(|l| !l.starts_with("Plate:"))
            .unwrap_or("")
            .to_string();
        if b.line.is_none() {
            b.line = Some(t.grid.header_row + 1);
        }
        let bare = title.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
        if bare.starts_with("Raw Data") {
            let ch = raw_channel(&title, mode);
            last_raw = Some(ch.clone());
            let i = b.channel(ch);
            push_grid(data_sheet, &t.grid, &mut b, i, None);
        } else if ["Layout", "Content", "Concentration", "Dilution"]
            .iter()
            .any(|k| title.contains(k))
        {
            let m = layout
                .entry(
                    title
                        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
                        .to_string(),
                )
                .or_default();
            for (r, c, cell) in t.grid.cells(data_sheet) {
                m.insert(well_name(r, c), cell.trimmed());
            }
        } else {
            let label = if title.is_empty() {
                "Unlabelled matrix".to_string()
            } else {
                title.clone()
            };
            let label = label
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
                .to_string();
            let i = b.channel(calculated(&label, last_raw.as_ref()));
            push_grid(data_sheet, &t.grid, &mut b, i, None);
        }
    }
    if !layout.is_empty() {
        b.extra.insert("layout".into(), json!(layout));
    }
    ex.blocks.push(b);
    ex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn mars_fluorescence() {
        let t = "User: USER,Path: C:\\Program Files (x86)\\BMG\\PHERAstar\\User\\Data,Test run no.: 4\nTest name: T,Date: 29/02/2016,Time: 14:34:46\n\nID1: black 384w,ID2: 20 ul\nFluorescence (FI)\n\nRaw Data (580/620)\n\n,1,2,3\nA,1,2,3\nB,4,5,\n";
        assert!(sniff(t));
        let ex = parse(&text_book(t.as_bytes()), false);
        assert_eq!(ex.model.as_deref(), Some("PHERAstar"));
        assert_eq!(ex.acquired_at.as_deref(), Some("2016-02-29T14:34:46"));
        let b = &ex.blocks[0];
        assert_eq!(b.plate, "black 384w");
        assert_eq!(b.channels[0].excitation_nm, Some(580.0));
        assert_eq!(b.channels[0].emission_nm, Some(620.0));
        assert_eq!(b.obs.len(), 5);
    }

    #[test]
    fn mars_table_view_spectrum() {
        // One pair per header line, a semicolon table with a wavelength line, an overflow cell.
        let t = "User: USER\nPath: C:\\Program Files (x86)\\BMG\\CLARIOstar\\User\\Data\nTest ID: 7\nTest Name: scan\nDate: 16.12.2021\nTime: 14:32:12\nAbsorbance spectrum;;;Absorbance values are displayed as OD\n\nWell;Content;Raw Data (Abs Spectrum);Raw Data (Abs Spectrum);Raw Data (Abs Spectrum)\n;Wavelength [nm];260;261;262\nE05;Sample X1;overflow;3.3;3.2\nF05;Sample X2;1;2;3\n";
        assert!(sniff(t));
        let ex = parse(&text_book(t.as_bytes()), false);
        let b = &ex.blocks[0];
        assert_eq!(b.read_type, Some(ReadType::Spectrum));
        assert_eq!(b.channels.len(), 1);
        assert_eq!(b.channels[0].mode, Mode::Absorbance);
        assert!(!b.channels[0].calculated);
        assert_eq!(
            b.channels[0].settings["scanned_wavelength"],
            json!("absorbance")
        );
        assert_eq!(b.obs.len(), 6);
        assert_eq!(b.obs[1].wavelength_nm, Some(261.0));
        assert!(b.obs[0].value.is_nan());
        assert_eq!(b.extra["layout"]["Content"]["F5"], json!("Sample X2"));
    }

    #[test]
    fn mars_table_view_calculated_and_endpoint() {
        let t = "User: U,Path: C:\\BMG\\CLARIOstar\\U\\Data,Test run no.: 1\nTest name: x,Date: 3/06/2026,Time: 11:38:46 AM\n\nFluorescence (FI) spectrum\n\nWell,Content,Average over replicates based on Raw Data (Ex Spectrum),Average over replicates based on Raw Data (Ex Spectrum)\n,Wavelength [nm],450,452\nB01,Sample X13,5891.7,5303.9\n";
        let ex = parse(&text_book(t.as_bytes()), false);
        let c = &ex.blocks[0].channels[0];
        assert!(c.calculated);
        assert_eq!(c.mode, Mode::Fluorescence);
        assert_eq!(c.settings["scanned_wavelength"], json!("excitation"));
        let t = "User: U,Path: C:\\BMG\\CLARIOstar\\U\\Data,Test run no.: 1\nTest name: x,Date: 3/06/2026,Time: 11:57:23 AM\n,,\nFluorescence (FI), multichromatic,,\n,,\nWell,Content,Raw Data (638-12/675-12 1),Raw Data (400-15/675-12 2)\nA01,Sample X1,420,702\nA02,Sample X2,369,584\n";
        let ex = parse(&text_book(t.as_bytes()), false);
        let b = &ex.blocks[0];
        assert_eq!(b.read_type, Some(ReadType::Endpoint));
        assert_eq!(b.channels.len(), 2);
        assert_eq!(b.channels[1].excitation_nm, Some(400.0));
        assert_eq!(b.channels[1].emission_nm, Some(675.0));
        assert_eq!(b.obs.len(), 4);
    }
}
