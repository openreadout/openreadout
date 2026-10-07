//! Tecan exports: i-control (infinite, Spark via the same layout) text/XLSX and Magellan XLSX.
//!
//! i-control: `Application: Tecan i-control`, `Device: …` / `Serial number: …`, `Date:`,
//! `Time:`, `Plate` (plate definition), then one section per `Label: <name>` with
//! `key … value … unit` settings, `Start Time:`, `Temperature: 23.1 °C`, and either a plate
//! matrix with a `<>` corner or a kinetic list (`Cycle Nr.`, `Time [s]`, `Temp. [°C]`, then
//! one line per well). With several reads per well (`Multiple Reads per Well (…)` `3 x 3`) the
//! data are a `Cycles / Well` block per well (kinetic) or a `Well`/`Mean`/`StDev` list (endpoint),
//! and each well's value is i-control's `Mean`. A workbook can hold one export per sheet (segments
//! of a run); each becomes a plate read. German exports (`Programm: Tecan i-control`, `Gerät:`,
//! `Datum:`, `Modus`, `Zyklen / Well`, `Mittelwert`) use the same layout.
//! Magellan: a list with a `Well positions` column and one value column per label (optional
//! time and temperature lines above the wells), followed by metadata lines (`Date of
//! measurement: …/Time of measurement: …`, method `.mth`, workspace `.wsp`, device, serial
//! number, per-measurement settings).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::grid::{find_grids, parse_well};
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType, first_number, nonempty};
use crate::sheet::{Book, Sheet};

/// The first cell of an i-control export: the application line, in English or German.
fn is_application_line(l: &str) -> bool {
    l.starts_with("Application: Tecan i-control")
        || l.starts_with("Application: SparkControl")
        || l.starts_with("Programm: Tecan i-control")
}

pub(crate) fn sniff_icontrol(text: &str) -> bool {
    let text = text.trim_start_matches('\u{feff}').trim_start();
    // SparkControl CSV exports open with `Method name: …` and name the application next.
    text.lines().take(3).any(|l| {
        let l = l.trim_start_matches('\u{feff}').trim_start();
        is_application_line(l)
    })
}

/// Whether a sheet starts with an i-control header.
fn has_application_line(s: &Sheet) -> bool {
    (0..s.rows.len().min(3)).any(|r| is_application_line(&s.text(r, 0)))
}

pub(crate) fn is_icontrol_book(book: &Book) -> bool {
    book.sheets.iter().any(has_application_line)
}

/// A header value under its English or its German key (German keys only where an export showed
/// them: docs/provenance/plate-readers.md, 2026-10-06).
fn get_any<'a>(ex: &'a Export, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| ex.get(k))
}

/// The line that opens one well's block of a kinetic read with several reads per well.
fn is_cycles_per_well(k: &str) -> bool {
    k == "Cycles / Well" || k == "Zyklen / Well"
}

/// The line of a multiple-read block or list that holds each well's mean.
fn is_mean(k: &str) -> bool {
    k == "Mean" || k == "Mittelwert"
}

/// The header of an endpoint list with several reads per well: `Well`, `Mean`, `StDev`, positions.
fn is_mean_list_header(sheet: &Sheet, r: usize) -> bool {
    sheet.text(r, 0) == "Well" && is_mean(&sheet.text(r, 1))
}

pub(crate) fn is_magellan_book(book: &Book) -> bool {
    book.sheets.iter().any(|s| {
        s.name.starts_with("Magellan")
            || ((0..s.rows.len().min(3))
                .any(|r| s.row_texts(r).iter().any(|t| t == "Well positions"))
                && (0..s.rows.len()).rev().take(40).any(|r| {
                    Path::new(&s.text(r, 0))
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("mth"))
                        || s.text(r, 0).starts_with("Date of measurement")
                }))
    })
}

/// `key … value [… unit]` → (key, value, unit).
fn setting(sheet: &Sheet, r: usize) -> Option<(String, String, Option<String>)> {
    let cells = sheet.row_texts(r);
    let key = cells.first()?.trim_end_matches(':').trim().to_string();
    let value = cells.get(1)?.clone();
    Some((key, value, cells.get(2).cloned()))
}

fn setting_key(k: &str) -> String {
    match k {
        "Excitation Wavelength" => "excitation_nm",
        "Emission Wavelength" => "emission_nm",
        "Measurement Wavelength" => "wavelength_nm",
        "Excitation Bandwidth" => "excitation_bandwidth_nm",
        "Emission Bandwidth" => "emission_bandwidth_nm",
        "Bandwidth" => "bandwidth_nm",
        "Gain" => "gain",
        "Number of Flashes" => "flashes",
        "Integration Time" => "integration_time_us",
        "Lag Time" => "lag_time_us",
        "Settle Time" => "settle_time_ms",
        "Part of Plate" => "part_of_plate",
        "Kinetic duration" => "kinetic_duration",
        "Interval Time" => "interval",
        // German exports (the keys one i-control 1.12 file showed)
        "Wellenlänge" => "wavelength_nm",
        "Bandbreite" => "bandwidth_nm",
        "Anzahl der Blitze" => "flashes",
        "Ruhezeit" => "settle_time_ms",
        "Intervallzeit" => "interval",
        other => {
            // `Multiple Reads per Well (Circle (filled))` → `3 x 3`; `… (Border)` → 1500 µm
            return match multiple_reads_pattern(other) {
                Some("Border" | "Rahmen") => "multiple_reads_border".into(),
                Some(_) => "multiple_reads".into(),
                None => other.to_string(),
            };
        }
    }
    .to_string()
}

/// What the parentheses of a `Multiple Reads per Well (…)` key hold: the read pattern
/// (`Circle (filled)`, `Quadrat`) or `Border`.
fn multiple_reads_pattern(key: &str) -> Option<&str> {
    ["Multiple Reads per Well (", "Mehrfachmessungen pro Well ("]
        .iter()
        .find_map(|p| key.strip_prefix(p).and_then(|x| x.strip_suffix(')')))
}

/// Header `key: value` pairs of an i-control sheet above its first section, in order.
fn header_pairs(sheet: &Sheet, header_end: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in 0..header_end {
        let cells = sheet.row_texts(r);
        let mut i = 0;
        while i < cells.len() {
            let c = &cells[i];
            if let Some((k, v)) = c.split_once(':')
                && !k.trim().is_empty()
                && k.len() < 48
            {
                let v = if v.trim().is_empty() {
                    match cells.get(i + 1).filter(|n| {
                        !n.split_once(':')
                            .is_some_and(|(k, _)| k.chars().any(char::is_alphabetic))
                    }) {
                        Some(n) => {
                            i += 1;
                            n.clone()
                        }
                        None => String::new(),
                    }
                } else {
                    v.trim().to_string()
                };
                out.push((
                    k.trim().to_string(),
                    v.trim().trim_start_matches('\'').to_string(),
                ));
            } else if i == 0 && cells.len() > 1 {
                out.push((c.clone(), cells[1..].join(" ")));
                break;
            }
            i += 1;
        }
        if cells.len() == 1 && !cells[0].contains(':') {
            out.push((cells[0].clone(), String::new()));
        }
    }
    out
}

pub(crate) fn parse_icontrol(book: &Book) -> Export {
    // A workbook can hold several complete exports, one per sheet (segments of one run).
    let exports: Vec<&Sheet> = book
        .sheets
        .iter()
        .filter(|s| {
            has_application_line(s)
                || (0..s.rows.len().min(3)).any(|r| s.text(r, 0).starts_with("Application:"))
        })
        .collect();
    let sheet = exports.first().copied().unwrap_or(&book.sheets[0]);
    let mut ex = Export::new(Kind::TecanIControl, book.container.clone());
    ex.sheets_read = if exports.is_empty() {
        vec![sheet.name.clone()]
    } else {
        exports.iter().map(|s| s.name.clone()).collect()
    };
    let n = sheet.rows.len();
    let first_label = (0..n)
        .find(|&r| sheet.text(r, 0).starts_with("Label:"))
        .unwrap_or(n);
    // i-control 1.11 CSV and SparkControl CSV exports have no `Label:` lines (tecan_csv.rs).
    let labelled = first_label < n;
    let header_end = if labelled {
        first_label
    } else {
        super::tecan_csv::header_end(sheet)
    };
    ex.header = header_pairs(sheet, header_end);
    ex.model = get_any(&ex, &["Device", "Gerät"]).map(str::to_string);
    ex.serial = get_any(&ex, &["Serial number", "Seriennummer"]).map(str::to_string);
    // `Tecan i-control , 1.9.17.0` next to `Application: Tecan i-control`
    ex.software_version = ex
        .header
        .iter()
        .find_map(|(k, v)| {
            (k == "Application" || k == "Programm")
                .then(|| v.rsplit(',').next().map(|s| s.trim().to_string()))
        })
        .flatten()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .or_else(|| {
            (0..3).find_map(|r| {
                sheet
                    .row_texts(r)
                    .iter()
                    .find_map(|t| t.split_once(" , ").map(|x| x.1.trim().to_string()))
            })
        })
        .or_else(|| super::tecan_csv::spark_version(sheet));
    ex.operator = get_any(&ex, &["User", "Anwender"]).map(str::to_string);
    if ex.protocol.is_none() {
        ex.protocol = ex.get("Method name").map(str::to_string);
    }
    if let Some(d) = get_any(&ex, &["Date", "Datum"]).map(str::to_string) {
        let t = get_any(&ex, &["Time", "Zeit"]).map(str::to_string);
        ex.set_acquired(&d, t.as_deref());
    }
    let plate_type = get_any(&ex, &["Plate", "Platte"]).map(str::to_string);
    let barcode = get_any(&ex, &["Plate-ID (Stacker)", "Platten-ID (Stapler)"]).map(str::to_string);
    let new_block = |started_at: Option<String>| {
        let mut b = Block::new("Plate 1", "Plate 1");
        b.plate_type = plate_type.clone();
        b.declared_wells = plate_type
            .as_deref()
            .and_then(first_number)
            .map(|n| n as u32)
            .filter(|n| crate::grid::dims_for_wells(*n).is_some());
        b.started_at = started_at;
        b.read_type = Some(ReadType::Endpoint);
        if let Some(id) = &barcode {
            b.barcode = Some(id.clone());
            b.plate.clone_from(id);
            b.name.clone_from(id);
        }
        b
    };
    if !labelled {
        let grids = find_grids(sheet);
        let mut b = new_block(ex.acquired_at.clone());
        super::tecan_csv::parse_unlabelled(sheet, &grids, header_end, &mut b);
        ex.blocks.push(b);
        return ex;
    }
    let several = exports.len() > 1;
    for (k, s) in exports.iter().enumerate() {
        let first = (0..s.rows.len())
            .find(|&r| s.text(r, 0).starts_with("Label:"))
            .unwrap_or(s.rows.len());
        // each sheet's own date and time (the first sheet's are the export's)
        let started = if k == 0 {
            ex.acquired_at.clone()
        } else {
            let pairs = header_pairs(s, first);
            let get = |keys: &[&str]| {
                pairs
                    .iter()
                    .find(|(pk, v)| keys.contains(&pk.as_str()) && !v.trim().is_empty())
                    .map(|(_, v)| v.clone())
            };
            get(&["Date", "Datum"])
                .and_then(|d| crate::datetime::combine(&d, get(&["Time", "Zeit"]).as_deref()))
                .map(|(iso, _)| iso)
        };
        let mut b = new_block(started);
        if several {
            b.name = format!("{} ({})", b.name, s.name);
            b.sheet = Some(s.name.clone());
        }
        parse_sections(s, first, &mut b, &mut ex.notes);
        ex.blocks.push(b);
    }
    if several {
        ex.notes.push(format!(
            "the workbook holds {} i-control exports, one per sheet (segments of a run); each is its own plate read with its own start time",
            exports.len()
        ));
    }
    ex
}

/// The `Label:` sections of one i-control sheet, from `first_label` on, into `b`.
fn parse_sections(sheet: &Sheet, first_label: usize, b: &mut Block, notes: &mut Vec<String>) {
    let n = sheet.rows.len();
    let grids = find_grids(sheet);
    let mut r = first_label;
    let mut temps = Vec::new();
    while r < n {
        let t0 = sheet.text(r, 0);
        if let Some(name) = t0.strip_prefix("Label:") {
            let name = name.trim().to_string();
            let mut settings: BTreeMap<String, Value> = BTreeMap::new();
            let mut mode_text = String::new();
            let mut kinetic = false;
            let mut started = None;
            let mut rr = r + 1;
            let end = (r + 1..n)
                .find(|&x| sheet.text(x, 0).starts_with("Label:"))
                .unwrap_or(n);
            // settings until the data
            while rr < end {
                let k = sheet.text(rr, 0);
                if k == "<>"
                    || k.starts_with("Cycle Nr")
                    || is_cycles_per_well(&k)
                    || is_mean_list_header(sheet, rr)
                    || grids.iter().any(|g| g.header_row == rr)
                {
                    break;
                }
                if k == "Kinetic Measurement" || k == "Kinetik - Messung" {
                    kinetic = true;
                }
                if k.starts_with("Start Time") || k.starts_with("Startzeit") {
                    started = sheet.row_texts(rr).get(1).cloned();
                } else if k == "Mode" || k == "Modus" {
                    mode_text = sheet.row_texts(rr).get(1).cloned().unwrap_or_default();
                } else if let Some(t) = sheet
                    .row_texts(rr)
                    .iter()
                    .find(|t| t.starts_with("Temperature:"))
                {
                    if let Some(v) = first_number(t) {
                        temps.push(v);
                    }
                } else if let Some((key, v, unit)) = setting(sheet, rr) {
                    if let Some(pattern) =
                        multiple_reads_pattern(&key).filter(|p| *p != "Border" && *p != "Rahmen")
                    {
                        settings.insert("multiple_reads_pattern".into(), json!(pattern));
                    }
                    let key = setting_key(&key);
                    let val = crate::sheet::parse_number(&v).map_or_else(|| json!(v), |x| json!(x));
                    settings.insert(key.clone(), val);
                    if let Some(u) = unit.filter(|_| {
                        !key.ends_with("_nm") && !key.ends_with("_us") && !key.ends_with("_ms")
                    }) {
                        settings.insert(format!("{key}_unit"), json!(u));
                    }
                }
                rr += 1;
            }
            // The section's `Mode` line names the mode; without one, a measurement wavelength
            // setting says absorbance.
            let (mode, basis) = if mode_text.is_empty() {
                if settings.contains_key("wavelength_nm") {
                    (Mode::Absorbance, crate::model::ModeBasis::Settings)
                } else {
                    (Mode::Unknown, crate::model::ModeBasis::Undetermined)
                }
            } else {
                (Mode::from_text(&mode_text), crate::model::ModeBasis::Stated)
            };
            let mut ch = Channel::derived(name.clone(), mode, basis);
            let num = |k: &str| settings.get(k).and_then(Value::as_f64);
            ch.wavelength_nm = num("wavelength_nm");
            ch.excitation_nm = num("excitation_nm");
            ch.emission_nm = num("emission_nm");
            ch.unit = Some(
                match mode {
                    Mode::Absorbance => "OD",
                    Mode::Luminescence => "RLU",
                    _ => "RFU",
                }
                .into(),
            );
            if !mode_text.is_empty() {
                settings.insert("mode".into(), json!(mode_text));
            }
            if let Some(s) = &started {
                settings.insert("start_time".into(), json!(s.trim_start_matches('\'')));
            }
            let multiple = settings.contains_key("multiple_reads");
            if multiple {
                settings.insert("well_value".into(), json!("mean of the reads per well"));
            }
            ch.settings = settings;
            let idx = b.channel(ch);
            if kinetic {
                b.read_type = Some(ReadType::Kinetic);
            }
            // data: one block per well or a list (several reads per well), a matrix, or a
            // kinetic list
            if let Some(cyc) = (rr..end).find(|&x| is_cycles_per_well(&sheet.text(x, 0))) {
                kinetic_per_well(sheet, cyc, end, b, idx);
                push_multiple_reads_note(notes);
            } else if let Some(h) = (rr..end).find(|&x| is_mean_list_header(sheet, x)) {
                mean_list(sheet, h, end, b, idx);
                push_multiple_reads_note(notes);
            } else if let Some(g) = grids
                .iter()
                .find(|g| g.header_row >= r && g.header_row < end)
            {
                super::push_grid(sheet, g, b, idx, None);
            } else if let Some(cyc) = (rr..end).find(|&x| sheet.text(x, 0).starts_with("Cycle Nr"))
            {
                kinetic_list(sheet, cyc, end, b, idx);
            }
            if b.line.is_none() {
                b.line = Some(r + 1);
            }
            r = end;
            continue;
        }
        r += 1;
    }
    if let Some(t) = temps.first() {
        b.temperature_c = Some(*t);
        if temps.len() > 1 {
            b.extra.insert("temperatures_c".into(), json!(temps));
        }
    }
}

fn push_multiple_reads_note(notes: &mut Vec<String>) {
    const NOTE: &str = "several reads per well: each well's value is i-control's Mean of its read positions; the position readings and StDev are not returned";
    if !notes.iter().any(|n| n == NOTE) {
        notes.push(NOTE.into());
    }
}

/// Kinetic reads with several reads per well: one block per well, `Cycles / Well`, then `<well>`
/// with the cycle numbers, `Time [s]`, `Temp. [°C]`, `Mean`, `StDev` and one line per read
/// position (`1;2`). Each well's value at a cycle is its `Mean`; a block without one is reported.
fn kinetic_per_well(sheet: &Sheet, start: usize, end: usize, b: &mut Block, ch: u32) {
    let mut r = start;
    let mut temps: Option<Vec<f64>> = None;
    let mut without_mean = Vec::new();
    while r < end {
        if !is_cycles_per_well(&sheet.text(r, 0)) {
            r += 1;
            continue;
        }
        let head = r + 1;
        let well_name = sheet.text(head, 0);
        let Some((pr, pc)) = parse_well(&well_name) else {
            r += 1;
            continue;
        };
        let width = sheet.row_len(head);
        let mut times: Vec<Option<f64>> = vec![None; width];
        let mut mean_row = None;
        let mut k = head + 1;
        while k < end {
            let t = sheet.text(k, 0);
            if t.is_empty() || is_cycles_per_well(&t) {
                break;
            }
            if t.starts_with("Time") || t.starts_with("Zeit") {
                for (c, slot) in times.iter_mut().enumerate().skip(1) {
                    *slot = sheet.cell(k, c).number();
                }
            } else if t.starts_with("Temp") && temps.is_none() {
                temps = Some(
                    (1..width)
                        .filter_map(|c| sheet.cell(k, c).number())
                        .collect(),
                );
            } else if is_mean(&t) {
                mean_row = Some(k);
            }
            k += 1;
        }
        match mean_row {
            Some(m) => {
                for (c, time) in times.iter().enumerate().skip(1) {
                    if sheet.cell(head, c).number().is_some() {
                        let cell = sheet.cell(m, c).clone();
                        b.push_cell(pr, pc, ch, *time, None, &cell);
                    }
                }
            }
            None => without_mean.push(well_name),
        }
        r = k;
    }
    if let Some(t) = temps.filter(|t| !t.is_empty()) {
        b.extra.insert("kinetic_temperatures_c".into(), json!(t));
    }
    if !without_mean.is_empty() {
        b.findings.push(openreadout_core::model::Finding::warning(
            "multiple_reads_without_mean",
            format!(
                "{}: {} well block(s) with several reads have no Mean line and were not read (first: {})",
                b.name,
                without_mean.len(),
                without_mean[0]
            ),
        ));
    }
}

/// Endpoint reads with several reads per well: `Well`, `Mean`, `StDev`, one column per read
/// position, then one line per well. Each well's value is its `Mean`.
fn mean_list(sheet: &Sheet, header: usize, end: usize, b: &mut Block, ch: u32) {
    for r in header + 1..end {
        let w = sheet.text(r, 0);
        let Some((pr, pc)) = parse_well(&w) else {
            break;
        };
        let cell = sheet.cell(r, 1).clone();
        b.push_cell(pr, pc, ch, None, None, &cell);
    }
}

/// `Cycle Nr.` / `Time [s]` / `Temp. [°C]` lines, then one line per well (`A1`, …).
pub(crate) fn kinetic_list(sheet: &Sheet, start: usize, end: usize, b: &mut Block, ch: u32) {
    let width = sheet.rows.get(start).map_or(0, Vec::len);
    let mut times: Vec<Option<f64>> = vec![None; width];
    let mut temps = Vec::new();
    let mut r = start + 1;
    while r < end {
        let k = sheet.text(r, 0);
        if k.starts_with("Time") {
            for (c, t) in times.iter_mut().enumerate().skip(1) {
                *t = sheet.cell(r, c).number();
            }
        } else if k.starts_with("Temp") {
            temps = (1..width)
                .filter_map(|c| sheet.cell(r, c).number())
                .collect();
        } else if let Some((pr, pc)) = parse_well(&k) {
            for (c, t) in times.iter().enumerate().skip(1) {
                let cell = sheet.cell(r, c).clone();
                if t.is_some() || !cell.is_blank() {
                    b.push_cell(pr, pc, ch, *t, None, &cell);
                }
            }
        } else if !k.is_empty() {
            break;
        }
        r += 1;
    }
    if !temps.is_empty() {
        b.extra
            .insert("kinetic_temperatures_c".into(), json!(temps));
    }
}

pub(crate) fn parse_magellan(book: &Book) -> Export {
    let sheet = book
        .sheets
        .iter()
        .find(|s| {
            (0..s.rows.len().min(3)).any(|r| s.row_texts(r).iter().any(|t| t == "Well positions"))
        })
        .unwrap_or(&book.sheets[0]);
    let mut ex = Export::new(Kind::TecanMagellan, book.container.clone());
    ex.sheets_read.push(sheet.name.clone());
    let n = sheet.rows.len();
    let header = (0..n.min(3))
        .find(|&r| sheet.row_texts(r).iter().any(|t| t == "Well positions"))
        .unwrap_or(0);
    let width = sheet.rows.get(header).map_or(0, Vec::len);
    let well_col = (0..width)
        .find(|&c| sheet.text(header, c) == "Well positions")
        .unwrap_or(0);
    let plate_col = (0..width).find(|&c| sheet.text(header, c) == "Plate");
    let value_cols: Vec<(usize, String)> = (0..width)
        .filter(|&c| c != well_col && Some(c) != plate_col)
        .map(|c| (c, sheet.text(header, c)))
        .filter(|(_, t)| !t.is_empty())
        .collect();
    let mut row_no = header + 1;
    let mut col_times: BTreeMap<usize, f64> = BTreeMap::new();
    let mut col_temps: BTreeMap<usize, f64> = BTreeMap::new();
    let mut plates: Vec<String> = Vec::new();
    let mut rows: Vec<(usize, (u32, u32))> = Vec::new();
    while row_no < n {
        let well_name = sheet.text(row_no, well_col);
        if let Some(pos) = parse_well(&well_name) {
            rows.push((row_no, pos));
            if let Some(pc) = plate_col
                && let Some(p) = nonempty(&sheet.text(row_no, pc))
                && !plates.contains(&p)
            {
                plates.push(p);
            }
        } else if well_name.is_empty() && rows.is_empty() {
            for (c, _) in &value_cols {
                let t = sheet.text(row_no, *c);
                if let Some(s) = t.strip_suffix('s').and_then(crate::sheet::parse_number) {
                    col_times.insert(*c, s);
                } else if (t.contains("°C") || t.ends_with('C'))
                    && let Some(v) = first_number(&t)
                {
                    col_temps.insert(*c, v);
                }
            }
        } else if !rows.is_empty() {
            break;
        }
        row_no += 1;
    }
    // Metadata lines after the data.
    let meta: Vec<String> = (row_no..n)
        .filter(|&x| !sheet.row_is_blank(x))
        .map(|x| sheet.row_line(x))
        .collect();
    let mut measurements: Vec<BTreeMap<String, String>> = Vec::new();
    let mut label_temps: BTreeMap<String, f64> = BTreeMap::new();
    let mut compact = BTreeMap::new();
    for (i, line) in meta.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Date of measurement:") {
            let (d, t) = rest
                .split_once("/Time of measurement:")
                .unwrap_or((rest, ""));
            ex.put("Date of measurement", d.trim());
            ex.put("Time of measurement", t.trim());
            ex.set_acquired(d.trim(), nonempty(t).as_deref());
        } else if Path::new(trimmed)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mth"))
        {
            if trimmed.contains(['\\', '/']) {
                ex.put("Method path", trimmed);
            } else {
                ex.put("Method", trimmed);
                ex.protocol = Some(trimmed.to_string());
            }
        } else if Path::new(trimmed)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wsp"))
        {
            if trimmed.contains(['\\', '/']) {
                ex.put("Workspace path", trimmed);
            } else {
                ex.put("Workspace", trimmed);
                ex.experiment = Some(trimmed.to_string());
            }
        } else if let Some(s) = trimmed.strip_prefix("Instrument serial number:") {
            ex.serial = nonempty(s);
            ex.put("Instrument serial number", s.trim());
            // the two lines before the serial number: user, then device
            if i >= 2 {
                ex.operator = Some(meta[i - 2].clone());
                ex.model = Some(meta[i - 1].clone());
                ex.put("User", meta[i - 2].clone());
                ex.put("Device", meta[i - 1].clone());
            }
        } else if trimmed.starts_with("Measurement ")
            && trimmed.ends_with(':')
            && first_number(trimmed).is_some()
        {
            measurements.push(BTreeMap::new());
        } else if let Some(rest) = trimmed.strip_prefix("Meas. temperature:") {
            if let Some((lab, t)) = rest.rsplit_once(':')
                && let Some(v) = first_number(t)
            {
                label_temps.insert(lab.trim().to_string(), v);
            }
        } else if let Some((k, v)) = trimmed.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if matches!(
                k.as_str(),
                "Measurement mode"
                    | "Measurement wavelength"
                    | "Number of flashes"
                    | "Plate definition file"
                    | "Unit"
                    | "Excitation wavelength"
                    | "Emission wavelength"
            ) {
                if measurements.is_empty() {
                    measurements.push(BTreeMap::new());
                }
                if let Some(m) = measurements.last_mut() {
                    m.insert(k, v);
                }
            } else {
                compact.insert(k.clone(), v.clone());
                ex.put(k, v);
            }
        } else if matches!(trimmed, "Absorbance" | "Fluorescence" | "Luminescence") {
            if measurements.is_empty() {
                measurements.push(BTreeMap::new());
            }
            if let Some(m) = measurements.last_mut() {
                m.insert("Measurement mode".into(), trimmed.to_string());
            }
        }
    }
    ex.sections.insert("metadata_lines".into(), json!(meta));
    let plate_desc = compact.get("Plate Description").cloned();
    let plate_file = measurements
        .iter()
        .find_map(|m| m.get("Plate definition file").cloned());
    let plate_name = plates
        .first()
        .cloned()
        .or_else(|| {
            plate_desc
                .as_deref()
                .and_then(|d| d.split('[').nth(1))
                .and_then(|x| x.split(']').next())
                .map(str::to_string)
        })
        .or_else(|| {
            plate_file
                .as_deref()
                .map(|f| f.split('.').next().unwrap_or(f).to_string())
        })
        .unwrap_or_else(|| "Plate 1".into());
    let mut block = Block::new(plate_name.clone(), plate_name);
    block.plate_type = plate_desc.or(plate_file);
    block.declared_wells = block
        .plate_type
        .as_deref()
        .and_then(first_number)
        .map(|n| n as u32)
        .filter(|n| crate::grid::dims_for_wells(*n).is_some());
    block.read_type = Some(if col_times.values().any(|t| *t > 0.0) {
        ReadType::Kinetic
    } else {
        ReadType::Endpoint
    });
    block.started_at.clone_from(&ex.acquired_at);
    block.line = Some(header + 1);
    block.sheet = (!sheet.name.is_empty()).then(|| sheet.name.clone());
    if plates.len() > 1 {
        block.extra.insert("plates".into(), json!(plates));
    }
    for (i, (c, label)) in value_cols.iter().enumerate() {
        let m = measurements.get(i).or_else(|| measurements.first());
        let mode = m
            .and_then(|m| m.get("Measurement mode"))
            .map_or(Mode::Unknown, |s| Mode::from_text(s));
        let mut ch = Channel::new(label.clone(), mode);
        if let Some(m) = m {
            ch.wavelength_nm = m
                .get("Measurement wavelength")
                .and_then(|v| first_number(v));
            ch.excitation_nm = m.get("Excitation wavelength").and_then(|v| first_number(v));
            ch.emission_nm = m.get("Emission wavelength").and_then(|v| first_number(v));
            if mode != Mode::Absorbance && ch.excitation_nm.is_some() {
                ch.wavelength_nm = None;
            }
            ch.unit = m.get("Unit").cloned();
            if let Some(f) = m
                .get("Number of flashes")
                .and_then(|v| crate::sheet::parse_number(v))
            {
                ch.settings.insert("flashes".into(), json!(f));
            }
        }
        let temp = label_temps
            .get(label)
            .copied()
            .or_else(|| col_temps.get(c).copied());
        if let Some(t) = temp {
            ch.settings.insert("temperature_c".into(), json!(t));
            if block.temperature_c.is_none() {
                block.temperature_c = Some(t);
            }
        }
        let idx = block.channel(ch);
        let time = col_times
            .get(c)
            .copied()
            .filter(|_| block.read_type == Some(ReadType::Kinetic));
        for &(row, (pr, pc)) in &rows {
            let cell = sheet.cell(row, *c).clone();
            block.push_cell(pr, pc, idx, time, None, &cell);
        }
    }
    ex.blocks.push(block);
    ex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn icontrol_text() {
        let t = "\u{feff}Application: Tecan i-control\t\t\t\tTecan i-control , 1.9.17.0\nDevice: infinite 200Pro\t\t\t\tSerial number: 1202007079\n\nDate:\t1/9/2014\nTime:\t'3:42:12 AM\n\nPlate\t\t\t\tCorning 96 Round Bottom [COS96rt.pdfx]\t\n\nLabel: MUF\nMode\t\t\t\tFluorescence Top Reading\t\nExcitation Wavelength\t\t\t\t360\tnm\nEmission Wavelength\t\t\t\t465\tnm\nStart Time:\t'1/9/2014 3:42:12 AM\n\n\tTemperature: 23.1 \u{b0}C\n<>\t1\t2\nA\t1221\t1463\nB\t1387\t1416\n\nEnd Time:\t'1/9/2014 3:42:39 AM\n";
        assert!(sniff_icontrol(t));
        let ex = parse_icontrol(&text_book(t.as_bytes()));
        assert_eq!(ex.model.as_deref(), Some("infinite 200Pro"));
        assert_eq!(ex.serial.as_deref(), Some("1202007079"));
        assert_eq!(ex.software_version.as_deref(), Some("1.9.17.0"));
        assert_eq!(ex.acquired_at.as_deref(), Some("2014-01-09T03:42:12"));
        let b = &ex.blocks[0];
        assert_eq!(b.channels[0].mode, Mode::Fluorescence);
        assert_eq!(b.channels[0].excitation_nm, Some(360.0));
        assert_eq!(b.obs.len(), 4);
        assert_eq!(b.temperature_c, Some(23.1));
        assert_eq!(b.declared_wells, Some(96));
    }

    /// The `Cycles / Well` layout of a kinetic read with several reads per well (tread's
    /// `time_series_multiple_reads.xlsx`, i-control 2.0), as tab-separated text.
    const MULTIREAD_KINETIC: &str = "Application: Tecan i-control\t\t\t\tTecan i-control , 2.0.10.0\nDevice: infinite 200Pro\t\t\t\tSerial number: 1234567890\n\nDate:\t1/1/2022\nTime:\t10:00:00 AM\n\nPlate\t\t\t\t96-well plate\n\nLabel: Label1\nKinetic Measurement\nMode\t\t\t\tAbsorbance\nMultiple Reads per Well (Circle (filled))\t\t\t\t3 x 3\nMultiple Reads per Well (Border)\t\t\t\t1500\t\u{b5}m\nMeasurement Wavelength\t\t\t\t600\tnm\nStart Time:\t1/1/2022 10:00:00 AM\n\nCycles / Well\nA1\t1\t2\nTime [s]\t0\t600\nTemp. [\u{b0}C]\t30.6\t30.3\nMean\t0.0959\t0.1082\nStDev\t0.0106\t0.0207\n1;2\t0.0897\t0.0925\n2;1\t0.1131\t0.1442\n\nCycles / Well\nB2\t1\t2\nTime [s]\t0\t600\nTemp. [\u{b0}C]\t30.6\t30.3\nMean\t0.0981\t\nStDev\t0.0139\t0.016\n1;2\t0.0956\t0.1001\n";

    #[test]
    fn several_reads_per_well_kinetic() {
        let ex = parse_icontrol(&text_book(MULTIREAD_KINETIC.as_bytes()));
        let b = &ex.blocks[0];
        assert_eq!(b.read_type, Some(ReadType::Kinetic));
        assert_eq!(b.channels[0].mode, Mode::Absorbance);
        assert_eq!(b.channels[0].wavelength_nm, Some(600.0));
        let s = &b.channels[0].settings;
        assert_eq!(s.get("multiple_reads"), Some(&json!("3 x 3")));
        assert_eq!(
            s.get("multiple_reads_pattern"),
            Some(&json!("Circle (filled)"))
        );
        assert_eq!(s.get("multiple_reads_border"), Some(&json!(1500.0)));
        // the Mean line, not a position line; B2's empty second mean is not a value
        let vals: Vec<(u32, u32, Option<f64>, f64)> = b
            .obs
            .iter()
            .map(|o| (o.row, o.col, o.time_s, o.value))
            .collect();
        assert_eq!(
            vals,
            vec![
                (0, 0, Some(0.0), 0.0959),
                (0, 0, Some(600.0), 0.1082),
                (1, 1, Some(0.0), 0.0981)
            ]
        );
        assert_eq!(
            b.extra.get("kinetic_temperatures_c"),
            Some(&json!([30.6, 30.3]))
        );
        assert!(ex.notes.iter().any(|n| n.contains("Mean")));
    }

    #[test]
    fn several_reads_per_well_endpoint_list() {
        let t = "Application: Tecan i-control\t\t\t\tTecan i-control , 2.0.10.0\nDevice: infinite 200Pro\n\nLabel: Label1\nMode\t\t\t\tAbsorbance\nMultiple Reads per Well (Circle (filled))\t\t\t\t3 x 3\nMeasurement Wavelength\t\t\t\t600\tnm\n\n\tTemperature: 30.6 \u{b0}C\nWell\tMean\tStDev\t1;2\t0;1\nA1\t0.1036\t0.0243\t0.096\t0.0896\nA2\t0.0951\t0.0073\t0.0913\t0.0913\n\nEnd Time:\t1/1/2022 10:01:00 AM\n";
        let ex = parse_icontrol(&text_book(t.as_bytes()));
        let b = &ex.blocks[0];
        let vals: Vec<f64> = b.obs.iter().map(|o| o.value).collect();
        assert_eq!(vals, vec![0.1036, 0.0951]);
        assert_eq!(b.temperature_c, Some(30.6));
    }

    #[test]
    fn german_export_is_recognised() {
        let t = "Programm: Tecan i-control\t\t\t\tTecan i-control , 1.12.4.0\nGer\u{e4}t: infinite 200Pro\t\t\t\tSeriennummer: 1610002981\n\nDatum:\t04.04.2019\nZeit:\t15:17:52\n\nAnwender\t\t\t\tKRZ\\L00616\nPlatte\t\t\t\tGreiner 96 Flat Bottom\n\nLabel: Label1\nKinetik - Messung\nModus\t\t\t\tAbsorption\nMehrfachmessungen pro Well (Quadrat)\t\t\t\t2 x 2\nWellenl\u{e4}nge\t\t\t\t600\tnm\nStartzeit:\t04.04.2019 15:17:54\n\nZyklen / Well\nA1\t1\t2\nZeit [s]\t0\t599.7\nTemp. [\u{b0}C]\t24.9\t37.1\nMittelwert\t0.1027\t0.1025\nStDev\t0.0104\t0.0097\n0;1\t0.0979\t0.0976\n";
        assert!(sniff_icontrol(t));
        let ex = parse_icontrol(&text_book(t.as_bytes()));
        assert_eq!(ex.model.as_deref(), Some("infinite 200Pro"));
        assert_eq!(ex.serial.as_deref(), Some("1610002981"));
        assert_eq!(ex.software_version.as_deref(), Some("1.12.4.0"));
        assert_eq!(ex.acquired_at.as_deref(), Some("2019-04-04T15:17:52"));
        assert_eq!(ex.operator.as_deref(), Some("KRZ\\L00616"));
        let b = &ex.blocks[0];
        assert_eq!(b.read_type, Some(ReadType::Kinetic));
        assert_eq!(b.channels[0].mode, Mode::Absorbance);
        assert_eq!(b.channels[0].wavelength_nm, Some(600.0));
        let vals: Vec<(Option<f64>, f64)> = b.obs.iter().map(|o| (o.time_s, o.value)).collect();
        assert_eq!(vals, vec![(Some(0.0), 0.1027), (Some(599.7), 0.1025)]);
    }
}
