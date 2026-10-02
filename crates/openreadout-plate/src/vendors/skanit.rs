//! Thermo Scientific SkanIt XLSX reports (Varioskan, Multiskan, Fluoroskan, Luminoskan).
//!
//! One sheet per protocol step: `Measurement results`, the session file name, the run time,
//! the step name (`Absorbance 1`, `Luminescence 1`, `Blank Subtraction 1`), `Wavelength: 450
//! nm` (or excitation/emission lines), then for each plate its name (`Plate 1`, `Blank plate`)
//! and a matrix whose corner names the quantity (`Abs`, `RLU`, `RFU`, `Blank subtracted`),
//! followed by a `Sample` layout matrix. Measurement steps are the sheets named after a
//! detection technology; every other step holds values SkanIt calculated. The `General
//! information`, `Session information`, `Instrument information` and `Protocol parameters`
//! sheets hold `key … value` lines.

use std::collections::BTreeMap;

use serde_json::json;

use super::{calculated, titled_grids};
use crate::grid::well_name;
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType, first_number};
use crate::sheet::{Book, Sheet};

pub(crate) fn is_skanit_book(book: &Book) -> bool {
    book.sheets.iter().any(|s| {
        s.text(0, 0) == "Measurement results"
            && (0..s.rows.len().min(12))
                .any(|r| s.text(r, 0).starts_with("Wavelength:") || s.text(r, 0).ends_with(" 1"))
    })
}

const INFO_SHEETS: [&str; 5] = [
    "General information",
    "Session information",
    "Instrument information",
    "Protocol parameters",
    "Run log",
];

fn info_pairs(sheet: &Sheet) -> Vec<(String, String)> {
    (0..sheet.rows.len())
        .filter_map(|r| {
            let cells = sheet.row_texts(r);
            match cells.as_slice() {
                [k, v, ..] => Some((k.clone(), v.clone())),
                _ => None,
            }
        })
        .collect()
}

fn step_mode(step: &str) -> Option<Mode> {
    let m = Mode::from_text(step);
    let l = step.to_ascii_lowercase();
    (m != Mode::Unknown
        && !l.contains("subtraction")
        && !l.contains("curve")
        && !l.contains("ratio"))
    .then_some(m)
    .or_else(|| l.starts_with("photometric").then_some(Mode::Absorbance))
}

pub(crate) fn parse(book: &Book) -> Export {
    let mut ex = Export::new(Kind::SkanIt, book.container.clone());
    for s in book
        .sheets
        .iter()
        .filter(|s| INFO_SHEETS.contains(&s.name.as_str()))
    {
        let pairs = info_pairs(s);
        if s.name == "Run log" {
            ex.sections.insert(
                "run_log".into(),
                json!(pairs.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>()),
            );
            continue;
        }
        let section: Vec<_> = pairs.iter().map(|(k, v)| json!([k, v])).collect();
        ex.sections.insert(
            s.name.to_ascii_lowercase().replace(' ', "_"),
            json!(section),
        );
        for (k, v) in pairs {
            ex.put(format!("{} / {k}", s.name), v);
        }
    }
    let find = |sheet: &str, key: &str| {
        ex.header
            .iter()
            .find(|(k, _)| k == &format!("{sheet} / {key}"))
            .map(|(_, v)| v.clone())
    };
    ex.model = find("Instrument information", "Name");
    ex.serial = find("Instrument information", "Serial number");
    ex.software_version = find("General information", "Report generated with SW version")
        .map(|v| v.rsplit("ver. ").next().unwrap_or(&v).trim().to_string());
    ex.experiment = find("Session information", "Session name");
    if let Some(t) = find("Session information", "Execution time") {
        ex.set_acquired(&t, None);
    }
    let mut blocks: BTreeMap<String, Block> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut last_raw: BTreeMap<String, Channel> = BTreeMap::new();
    for s in book
        .sheets
        .iter()
        .filter(|s| s.text(0, 0) == "Measurement results")
    {
        let step = s.text(4, 0);
        let mode = step_mode(&step);
        let when = s.text(2, 0);
        let mut current_plate = String::from("Plate 1");
        let mut prev_end = 0;
        for mut t in titled_grids(s) {
            // SkanIt titles live in column A; side tables (averages, curve points) share the lines.
            t.titles = (prev_end..t.grid.header_row)
                .map(|r| s.text(r, 0))
                .filter(|l| !l.is_empty())
                .collect();
            prev_end = t.grid.end_row;
            if t.grid.corner == "Sample" {
                if let Some(b) = blocks.get_mut(&current_plate) {
                    let mut m: BTreeMap<String, String> = BTreeMap::new();
                    for (r, c, cell) in t.grid.cells(s) {
                        m.insert(well_name(r, c), cell.trimmed());
                    }
                    b.extra.entry("layout".into()).or_insert(json!(m));
                }
                continue;
            }
            // Titles above the matrix, nearest last: … step, wavelength, plate name.
            let plate = t
                .titles
                .iter()
                .rev()
                .find(|l| {
                    let l = l.trim();
                    !l.is_empty()
                        && !l.starts_with("Wavelength")
                        && !l.starts_with("Excitation")
                        && !l.starts_with("Emission")
                        && *l != step
                        && !l.contains(':')
                })
                .cloned()
                .unwrap_or_else(|| current_plate.clone());
            current_plate.clone_from(&plate);
            let wl_line = t.find(|l| l.starts_with("Wavelength")).map(str::to_string);
            let ex_line = t.find(|l| l.starts_with("Excitation")).map(str::to_string);
            let em_line = t.find(|l| l.starts_with("Emission")).map(str::to_string);
            let b = blocks.entry(plate.clone()).or_insert_with(|| {
                order.push(plate.clone());
                let mut b = Block::new(plate.clone(), plate.clone());
                b.read_type = Some(ReadType::Endpoint);
                b.sheet = Some(s.name.clone());
                b.line = Some(t.grid.header_row + 1);
                if let Some(w) = crate::datetime::combine(&when, None) {
                    b.started_at = Some(w.0);
                }
                b
            });
            let label = format!("{step} ({})", t.grid.corner);
            let ch = if let Some(m) = mode {
                {
                    let mut c = Channel::new(label, m);
                    let wl = wl_line
                        .as_deref()
                        .and_then(first_number)
                        .filter(|w| *w > 0.0);
                    match m {
                        Mode::Absorbance => c.wavelength_nm = wl,
                        Mode::Luminescence => c.emission_nm = wl,
                        _ => {
                            c.excitation_nm = ex_line.as_deref().and_then(first_number);
                            c.emission_nm = em_line.as_deref().and_then(first_number).or(wl);
                        }
                    }
                    c.unit = Some(t.grid.corner.clone()).filter(|u| !u.is_empty());
                    c.settings.insert("step".into(), json!(step));
                    last_raw.insert(plate.clone(), c.clone());
                    c
                }
            } else {
                {
                    let mut c = calculated(&label, last_raw.get(&plate));
                    c.settings.insert("step".into(), json!(step));
                    c
                }
            };
            let idx = b.channel(ch);
            super::push_grid(s, &t.grid, b, idx, None);
        }
    }
    if let Some(s) = book.sheets.iter().find(|s| s.name == "Layout definitions") {
        for (plate, defs) in layout_definitions(s) {
            let target = if blocks.contains_key(&plate) {
                Some(plate)
            } else {
                // one plate: attach whatever its name
                (blocks.len() == 1)
                    .then(|| blocks.keys().next().cloned())
                    .flatten()
            };
            if let Some(b) = target.and_then(|p| blocks.get_mut(&p)) {
                b.extra.insert("layout_definitions".into(), json!(defs));
            }
        }
    }
    for k in order {
        if let Some(b) = blocks.remove(&k) {
            ex.blocks.push(b);
        }
    }
    ex
}

/// Title → well → text.
type TitledMaps = BTreeMap<String, BTreeMap<String, String>>;

/// The `Layout definitions` sheet: per plate (`Name` line), a header of column numbers, then per
/// row the sample names (after the row letter), the group and the concentration or dilution
/// (lines with an empty first cell). Returns plate → title (`Sample`, `Group`, `Conc/Dil`) →
/// well → text.
fn layout_definitions(s: &Sheet) -> Vec<(String, TitledMaps)> {
    const TITLES: [&str; 3] = ["Sample", "Group", "Conc/Dil"];
    let mut out = Vec::new();
    let mut plate = String::from("Plate 1");
    let mut r = 0;
    while r < s.rows.len() {
        let first = s.text(r, 0);
        if first == "Name" {
            plate = s.text(r, 1);
            r += 1;
            continue;
        }
        // header: empty first cell, then 1, 2, 3 …
        let cols: Vec<(usize, u32)> = (1..s.rows[r].len())
            .map_while(|c| {
                let t = s.text(r, c);
                let v: f64 = t.parse().ok()?;
                (v >= 1.0 && v.fract() == 0.0 && v < 10_000.0).then_some((c, v as u32))
            })
            .collect();
        if !first.is_empty() || cols.len() < 2 || cols[0].1 != 1 {
            r += 1;
            continue;
        }
        let mut defs: TitledMaps = BTreeMap::new();
        r += 1;
        let mut row_idx: Option<u32> = None;
        let mut sub = 0usize;
        while r < s.rows.len() {
            let lead = s.text(r, 0);
            if lead.len() <= 2 && !lead.is_empty() && lead.chars().all(|c| c.is_ascii_alphabetic())
            {
                row_idx = crate::grid::parse_row_label(&lead, false);
                sub = 0;
            } else if lead.is_empty() && row_idx.is_some() && s.row_len(r) > 0 {
                sub += 1;
            } else {
                break;
            }
            if let (Some(ri), Some(title)) = (row_idx, TITLES.get(sub)) {
                for &(c, n) in &cols {
                    let t = s.text(r, c);
                    if !t.is_empty() {
                        defs.entry((*title).to_string())
                            .or_default()
                            .insert(well_name(ri, n - 1), t);
                    }
                }
            }
            r += 1;
        }
        if !defs.is_empty() {
            out.push((plate.clone(), defs));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Cell;

    fn row(cells: &[&str]) -> Vec<Cell> {
        cells
            .iter()
            .map(|c| match c.parse::<f64>() {
                Ok(v) => Cell::Number(v),
                Err(_) if c.is_empty() => Cell::Empty,
                Err(_) => Cell::Text((*c).to_string()),
            })
            .collect()
    }

    #[test]
    fn layout_definitions_sheet() {
        let s = Sheet {
            name: "Layout definitions".into(),
            rows: vec![
                row(&["Name", "Plate 1"]),
                row(&["Plate template", "ANSI/SBS Standard, 96-well"]),
                row(&["", "1.0", "2.0", "3.0"]),
                row(&["A", "Std0001", "Std0001", "Un0001"]),
                row(&["", "Group 1", "Group 1", "Group 1"]),
                row(&["", "1 microg/ml", "1 microg/ml", "1:2"]),
                row(&["B", "Blank1", "Blank1", "Un0002"]),
                row(&["", "Group 1", "Group 1", "Group 1"]),
                row(&["", "", "", "1:25"]),
                row(&[" "]),
            ],
        };
        let defs = layout_definitions(&s);
        assert_eq!(defs.len(), 1);
        let (plate, d) = &defs[0];
        assert_eq!(plate, "Plate 1");
        assert_eq!(d["Sample"]["A1"], "Std0001");
        assert_eq!(d["Sample"]["B3"], "Un0002");
        assert_eq!(d["Group"]["B2"], "Group 1");
        assert_eq!(d["Conc/Dil"]["A2"], "1 microg/ml");
        assert_eq!(d["Conc/Dil"]["B3"], "1:25");
        assert!(!d["Conc/Dil"].contains_key("B1"));
    }
}
