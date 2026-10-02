//! Tecan exports: i-control (infinite, Spark via the same layout) text/XLSX and Magellan XLSX.
//!
//! i-control: `Application: Tecan i-control`, `Device: …` / `Serial number: …`, `Date:`,
//! `Time:`, `Plate` (plate definition), then one section per `Label: <name>` with
//! `key … value … unit` settings, `Start Time:`, `Temperature: 23.1 °C`, and either a plate
//! matrix with a `<>` corner or a kinetic list (`Cycle Nr.`, `Time [s]`, `Temp. [°C]`, then
//! one line per well).
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

pub(crate) fn sniff_icontrol(text: &str) -> bool {
    let text = text.trim_start_matches('\u{feff}').trim_start();
    // SparkControl CSV exports open with `Method name: …` and name the application next.
    text.lines().take(3).any(|l| {
        let l = l.trim_start_matches('\u{feff}').trim_start();
        l.starts_with("Application: Tecan i-control") || l.starts_with("Application: SparkControl")
    })
}

pub(crate) fn is_icontrol_book(book: &Book) -> bool {
    book.sheets.iter().any(|s| {
        (0..s.rows.len().min(3)).any(|r| {
            s.text(r, 0).starts_with("Application: Tecan i-control")
                || s.text(r, 0).starts_with("Application: SparkControl")
        })
    })
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
        other => return other.to_string(),
    }
    .to_string()
}

pub(crate) fn parse_icontrol(book: &Book) -> Export {
    let sheet = book
        .sheets
        .iter()
        .find(|s| (0..s.rows.len().min(3)).any(|r| s.text(r, 0).starts_with("Application:")))
        .unwrap_or(&book.sheets[0]);
    let mut ex = Export::new(Kind::TecanIControl, book.container.clone());
    let n = sheet.rows.len();
    let grids = find_grids(sheet);
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
                ex.put(k.trim(), v.trim().trim_start_matches('\'').to_string());
            } else if i == 0 && cells.len() > 1 {
                ex.put(c.clone(), cells[1..].join(" "));
                break;
            }
            i += 1;
        }
        if cells.len() == 1 && !cells[0].contains(':') {
            ex.put(cells[0].clone(), String::new());
        }
    }
    ex.model = ex.get("Device").map(str::to_string);
    ex.serial = ex.get("Serial number").map(str::to_string);
    // `Tecan i-control , 1.9.17.0` next to `Application: Tecan i-control`
    ex.software_version = ex
        .header
        .iter()
        .find_map(|(k, v)| {
            (k == "Application").then(|| v.rsplit(',').next().map(|s| s.trim().to_string()))
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
    ex.operator = ex.get("User").map(str::to_string);
    if ex.protocol.is_none() {
        ex.protocol = ex.get("Method name").map(str::to_string);
    }
    if let Some(d) = ex.get("Date").map(str::to_string) {
        let t = ex.get("Time").map(str::to_string);
        ex.set_acquired(&d, t.as_deref());
    }
    let plate_type = ex.get("Plate").map(str::to_string);
    let mut b = Block::new("Plate 1", "Plate 1");
    b.plate_type = plate_type.clone();
    b.declared_wells = plate_type
        .as_deref()
        .and_then(first_number)
        .map(|n| n as u32)
        .filter(|n| crate::grid::dims_for_wells(*n).is_some());
    b.started_at.clone_from(&ex.acquired_at);
    b.read_type = Some(ReadType::Endpoint);
    if let Some(id) = ex.get("Plate-ID (Stacker)").map(str::to_string) {
        b.barcode = Some(id.clone());
        b.plate.clone_from(&id);
        b.name = id;
    }
    if !labelled {
        super::tecan_csv::parse_unlabelled(sheet, &grids, header_end, &mut b);
        ex.blocks.push(b);
        return ex;
    }
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
                    || grids.iter().any(|g| g.header_row == rr)
                {
                    break;
                }
                if k == "Kinetic Measurement" {
                    kinetic = true;
                }
                if k.starts_with("Start Time") {
                    started = sheet.row_texts(rr).get(1).cloned();
                } else if k == "Mode" {
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
            ch.settings = settings;
            let idx = b.channel(ch);
            if kinetic {
                b.read_type = Some(ReadType::Kinetic);
            }
            // data: a matrix, or a kinetic list
            if let Some(g) = grids
                .iter()
                .find(|g| g.header_row >= r && g.header_row < end)
            {
                super::push_grid(sheet, g, &mut b, idx, None);
            } else if let Some(cyc) = (rr..end).find(|&x| sheet.text(x, 0).starts_with("Cycle Nr"))
            {
                kinetic_list(sheet, cyc, end, &mut b, idx);
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
    ex.blocks.push(b);
    ex
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
}
