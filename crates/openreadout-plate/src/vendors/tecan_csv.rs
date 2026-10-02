//! Tecan exports without `Label:` lines: the i-control 1.11 comma-delimited CSV and the
//! SparkControl CSV (`docs/formats/plate-readers.md` § Tecan i-control and Magellan).
//!
//! Both put the read settings in `Mode` groups (`Mode,,,,Absorbance` / `Wavelength,,,,595,nm`;
//! SparkControl: `Mode,Absorbance` / `Name,OD600` / `Measurement wavelength,,,,600,nm`) and the
//! data in tables that start with `Cycle Nr.` (kinetic) or `<>` (endpoint):
//! - i-control 1.11: a title line `<name>:<suffix>` (`OD600:600`), then one line per cycle:
//!   `Cycle Nr.,Time [s],Temp. [°C],A1,A2,…`;
//! - SparkControl kinetic: a title line `<name>`, then `Cycle Nr.,1,2,…`, `Time [s],…`,
//!   `Temp. [°C],…` and one line per well;
//! - SparkControl endpoint: `<>,Value,Time [ms]` and one line per well; a `<>` plate matrix is
//!   read like the tab export's.
//!
//! A table belongs to the `Mode` group whose `Name` equals its title, else to the next unused
//! group in order.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::grid::{Grid, parse_well};
use crate::model::{Block, Channel, Mode, ReadType};
use crate::sheet::{Sheet, parse_number};

/// One `Mode` group: a read's mode, name and settings.
#[derive(Debug, Clone)]
struct ReadDef {
    mode_text: String,
    name: Option<String>,
    settings: BTreeMap<String, Value>,
    used: bool,
}

/// A data table and the title line above it, if any.
#[derive(Debug, Clone)]
struct Table {
    /// Row of the `Cycle Nr.` / `<>` header.
    header: usize,
    title: Option<String>,
}

/// Where the header key/value lines end: the first `List of actions…`, `Mode` or table line.
pub(crate) fn header_end(sheet: &Sheet) -> usize {
    (0..sheet.rows.len())
        .find(|&r| {
            let t = sheet.text(r, 0);
            t.starts_with("List of actions") || t == "Mode" || is_table_header(sheet, r)
        })
        .unwrap_or(sheet.rows.len())
}

fn is_table_header(sheet: &Sheet, r: usize) -> bool {
    let t = sheet.text(r, 0);
    t.starts_with("Cycle Nr") || t == "<>"
}

/// SparkControl writes its version in the cell after `Application: SparkControl` (`V2.3`).
pub(crate) fn spark_version(sheet: &Sheet) -> Option<String> {
    (0..sheet.rows.len().min(4)).find_map(|r| {
        let cells = sheet.row_texts(r);
        let at = cells
            .iter()
            .position(|c| c.starts_with("Application:") && c.contains("SparkControl"))?;
        let v = cells.get(at + 1)?;
        let v = v.strip_prefix('V').unwrap_or(v);
        v.chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
            .then(|| v.to_string())
    })
}

/// Our name for a setting key (case-insensitive; i-control 1.11 and SparkControl spellings).
fn key_name(k: &str) -> String {
    let l = k.trim().trim_end_matches(':').to_ascii_lowercase();
    match l.as_str() {
        "wavelength" | "measurement wavelength" => "wavelength_nm",
        "excitation wavelength" => "excitation_nm",
        "emission wavelength" => "emission_nm",
        "excitation bandwidth" => "excitation_bandwidth_nm",
        "emission bandwidth" => "emission_bandwidth_nm",
        "bandwidth" => "bandwidth_nm",
        "gain" => "gain",
        "number of flashes" => "flashes",
        "integration time" => "integration_time",
        "lag time" => "lag_time",
        "settle time" => "settle_time",
        "part of plate" => "part_of_plate",
        "kinetic duration" => "kinetic_duration",
        "interval time" => "interval",
        "kinetic cycles" => "kinetic_cycles",
        "attenuation" => "attenuation",
        "z-position" => "z_position",
        "z-position mode" => "z_position_mode",
        "mirror" => "mirror",
        "excitation" => "excitation",
        "emission" => "emission",
        _ => return k.trim().trim_end_matches(':').to_string(),
    }
    .to_string()
}

/// Fold a stated unit into the key the tab export uses (`settle_time` + `ms` → `settle_time_ms`,
/// `integration_time` + `µs` → `integration_time_us`; `…_nm` keys drop `nm`); a gain's `Manual`
/// or `Optimal` becomes `gain_mode`; any other unit is kept as `<key>_unit`.
fn with_unit(key: &str, unit: Option<&str>) -> (String, Option<(String, String)>) {
    let Some(unit) = unit.map(str::trim).filter(|u| !u.is_empty()) else {
        return (key.to_string(), None);
    };
    let suffix = match unit {
        "µs" | "us" | "\u{3bc}s" => Some("us"),
        "ms" => Some("ms"),
        "s" => Some("s"),
        "µm" | "um" | "\u{3bc}m" => Some("um"),
        "mm" => Some("mm"),
        _ => None,
    };
    if key.ends_with("_nm") && unit == "nm" {
        (key.to_string(), None)
    } else if key == "gain" {
        (
            key.to_string(),
            Some(("gain_mode".into(), unit.to_string())),
        )
    } else if let Some(s) = suffix {
        (format!("{key}_{s}"), None)
    } else {
        (
            key.to_string(),
            Some((format!("{key}_unit"), unit.to_string())),
        )
    }
}

/// The `Mode` groups after `from` (SparkControl endpoint exports put each group right above
/// its table; the other layouts put every group before the first table).
fn read_defs(sheet: &Sheet, from: usize) -> (Vec<ReadDef>, bool) {
    let mut defs: Vec<ReadDef> = Vec::new();
    let mut kinetic = false;
    let mut in_group = false;
    for r in from..sheet.rows.len() {
        if is_table_header(sheet, r) {
            in_group = false;
            continue;
        }
        let cells = sheet.row_texts(r);
        let Some(first) = cells.first() else {
            // a group's settings are contiguous lines
            in_group = false;
            continue;
        };
        if first == "Kinetic Measurement" || first == "Kinetic Cycles" {
            kinetic = true;
        }
        if first == "Mode" {
            let mode_text = cells.get(1).cloned().unwrap_or_default();
            if mode_text.eq_ignore_ascii_case("Kinetic") {
                // SparkControl's kinetic settings are a group of their own.
                kinetic = true;
                in_group = false;
                continue;
            }
            defs.push(ReadDef {
                mode_text,
                name: None,
                settings: BTreeMap::new(),
                used: false,
            });
            in_group = true;
            continue;
        }
        if !in_group {
            continue;
        }
        let Some(def) = defs.last_mut() else {
            continue;
        };
        if first == "Name" {
            def.name = cells.get(1).cloned();
            continue;
        }
        if first.starts_with("Start Time")
            || first.starts_with("Temperature")
            || first.starts_with("End Time")
        {
            continue;
        }
        if let Some(value) = cells.get(1) {
            let val = parse_number(value).map_or_else(|| json!(value), |x| json!(x));
            let (key, extra) = with_unit(&key_name(first), cells.get(2).map(String::as_str));
            def.settings.insert(key, val);
            if let Some((k, v)) = extra {
                def.settings.insert(k, json!(v));
            }
        }
    }
    (defs, kinetic)
}

/// Every data table after `from`, with its title line.
fn tables(sheet: &Sheet, from: usize) -> Vec<Table> {
    let mut out = Vec::new();
    for r in from..sheet.rows.len() {
        if !is_table_header(sheet, r) {
            continue;
        }
        // A `<>` line directly under a `Cycle Nr.` block would be a matrix of that block.
        let title = r
            .checked_sub(1)
            .filter(|&p| sheet.row_len(p) == 1 && !is_table_header(sheet, p))
            .map(|p| sheet.text(p, 0))
            .filter(|t| {
                !t.is_empty()
                    && !t.starts_with("Start Time")
                    && !t.starts_with("Temperature")
                    && !t.starts_with("End Time")
            });
        out.push(Table { header: r, title });
    }
    out
}

/// Read the sections of an export without `Label:` lines into `b`.
pub(crate) fn parse_unlabelled(sheet: &Sheet, grids: &[Grid], from: usize, b: &mut Block) {
    let tables = tables(sheet, from);
    let (mut defs, kinetic) = read_defs(sheet, from);
    if kinetic {
        b.read_type = Some(ReadType::Kinetic);
    }
    let mut temps = Vec::new();
    let mut starts = Vec::new();
    for r in from..sheet.rows.len() {
        let cells = sheet.row_texts(r);
        match cells.first().map(String::as_str) {
            Some(t) if t.starts_with("Start Time") => {
                if let Some(v) = cells.get(1) {
                    starts.push(v.clone());
                }
            }
            Some(t) if t.starts_with("Temperature") && !t.starts_with("Temperature control") => {
                if let Some(v) = cells.get(1).and_then(|v| parse_number(v)) {
                    temps.push(v);
                }
            }
            _ => {}
        }
    }
    for (k, t) in tables.iter().enumerate() {
        let name = t
            .title
            .as_deref()
            .map(|s| s.split_once(':').map_or(s, |(a, _)| a).trim().to_string());
        let pick = name
            .as_deref()
            .and_then(|n| {
                defs.iter()
                    .position(|d| !d.used && d.name.as_deref() == Some(n))
            })
            .or_else(|| defs.iter().position(|d| !d.used));
        let def = pick.map(|i| {
            defs[i].used = true;
            defs[i].clone()
        });
        let label = name
            .clone()
            .or_else(|| def.as_ref().and_then(|d| d.name.clone()))
            .unwrap_or_else(|| format!("Read {}", k + 1));
        let mode_text = def
            .as_ref()
            .map(|d| d.mode_text.clone())
            .unwrap_or_default();
        let mut settings = def.map(|d| d.settings).unwrap_or_default();
        // The group's `Mode` line names the mode; without one, a `Measurement wavelength`
        // setting (absorbance) or the read's name decides it.
        let (mode, basis) = if mode_text.is_empty() {
            if settings.contains_key("wavelength_nm") {
                (Mode::Absorbance, crate::model::ModeBasis::Settings)
            } else {
                (Mode::from_text(&label), crate::model::ModeBasis::Label)
            }
        } else {
            (Mode::from_text(&mode_text), crate::model::ModeBasis::Stated)
        };
        let mut ch = Channel::derived(label, mode, basis);
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
        if let Some(title) = &t.title
            && Some(title) != name.as_ref()
        {
            settings.insert("section_title".into(), json!(title));
        }
        ch.settings = settings;
        let idx = b.channel(ch);
        let end = tables.get(k + 1).map_or(sheet.rows.len(), |n| n.header);
        let head = sheet.text(t.header, 0);
        if head.starts_with("Cycle Nr") {
            if sheet.text(t.header, 1).starts_with("Time") {
                cycles_as_rows(sheet, t.header, end, b, idx);
            } else {
                super::tecan::kinetic_list(sheet, t.header, end, b, idx);
            }
            b.read_type = Some(ReadType::Kinetic);
        } else if let Some(g) = grids.iter().find(|g| g.header_row == t.header) {
            super::push_grid(sheet, g, b, idx, None);
        } else if sheet.text(t.header, 1).eq_ignore_ascii_case("Value") {
            well_list(sheet, t.header, end, b, idx);
        }
        if b.line.is_none() {
            b.line = Some(t.header + 1);
        }
    }
    if let Some(t) = temps.first() {
        b.temperature_c = Some(*t);
        if temps.len() > 1 {
            b.extra.insert("temperatures_c".into(), json!(temps));
        }
    }
    if let Some(s) = starts.first() {
        b.extra
            .insert("start_time".into(), json!(s.trim_start_matches('\'')));
    }
}

/// `Cycle Nr.,Time [s],Temp. [°C],A1,A2,…` then one line per cycle (i-control 1.11 CSV).
fn cycles_as_rows(sheet: &Sheet, header: usize, end: usize, b: &mut Block, ch: u32) {
    let width = sheet.rows.get(header).map_or(0, Vec::len);
    let wells: Vec<(usize, (u32, u32))> = (0..width)
        .filter_map(|c| parse_well(&sheet.text(header, c)).map(|w| (c, w)))
        .collect();
    let time_col = (0..width).find(|&c| sheet.text(header, c).starts_with("Time"));
    let temp_col = (0..width).find(|&c| sheet.text(header, c).starts_with("Temp"));
    let mut temps = Vec::new();
    for r in header + 1..end {
        // a cycle line starts with its number; anything else ends the table
        if sheet.cell(r, 0).number().is_none() {
            break;
        }
        let t = time_col.and_then(|c| sheet.cell(r, c).number());
        if let Some(v) = temp_col.and_then(|c| sheet.cell(r, c).number()) {
            temps.push(v);
        }
        for &(c, (pr, pc)) in &wells {
            let cell = sheet.cell(r, c).clone();
            b.push_cell(pr, pc, ch, t, None, &cell);
        }
    }
    if !temps.is_empty() {
        b.extra
            .insert("kinetic_temperatures_c".into(), json!(temps));
    }
}

/// `<>,Value,Time [ms]` then `A1,0.0921,0` lines (SparkControl endpoint).
fn well_list(sheet: &Sheet, header: usize, end: usize, b: &mut Block, ch: u32) {
    for r in header + 1..end {
        let Some((pr, pc)) = parse_well(&sheet.text(r, 0)) else {
            if sheet.row_is_blank(r) {
                continue;
            }
            break;
        };
        let cell = sheet.cell(r, 1).clone();
        b.push_cell(pr, pc, ch, None, None, &cell);
    }
}

#[cfg(test)]
mod tests {
    use crate::sheet::text_book;

    fn parse(text: &str) -> crate::model::Export {
        let book = text_book(text.as_bytes());
        let mut ex = super::super::tecan::parse_icontrol(&book);
        ex.finish();
        ex
    }

    #[test]
    fn icontrol_csv_cycles_as_rows() {
        let text = "\u{feff}Application: Tecan i-control,,,,\"Tecan i-control , 1.11.1.0\",,\n\
Device: infinite 200Pro,,,,Serial number: 1409002252,,\n\
Date:,14.10.2016,,,,,\n\
Plate,,,,Greiner 96 Flat Bottom,,\n\
List of actions in this measurement script:,,,,,,\n\
Kinetic Measurement,,,,,,\n\
Kinetic Cycles,,,,2,,\n\
Mode,,,,Absorbance,,\n\
Wavelength,,,,595,nm,\n\
Mode,,,,Luminescence,,\n\
Integration Time,,,,1000,ms,\n\
Start Time:,14.10.2016 14:11:38,,,,,\n\
,,,,,,\n\
OD600:600,,,,,,\n\
Cycle Nr.,Time [s],Temp. [°C],A1,A2,B1\n\
1,0,37.2,0.1,0.2,0.3\n\
2,300,37,0.4,0.5,OVER\n\
,,,,,,\n\
LUMI:Lum,,,,,,\n\
Cycle Nr.,Time [s],Temp. [°C],A1,A2,B1\n\
1,0,37.6,-4,-2,-5\n\
2,12.8,37.6,-2,4,2\n\
,,,,,,\n\
End Time:,15.10.2016 00:11:32,,,,,\n";
        let ex = parse(text);
        assert_eq!(ex.software_version.as_deref(), Some("1.11.1.0"));
        assert_eq!(ex.model.as_deref(), Some("infinite 200Pro"));
        let b = &ex.blocks[0];
        assert_eq!(b.channels.len(), 2);
        assert_eq!(b.channels[0].label, "OD600");
        assert_eq!(b.channels[0].mode, crate::model::Mode::Absorbance);
        assert_eq!(b.channels[0].wavelength_nm, Some(595.0));
        assert_eq!(b.channels[1].mode, crate::model::Mode::Luminescence);
        assert_eq!(b.obs.len(), 12);
        let over = b.obs.iter().find(|o| o.text.is_some()).unwrap();
        assert_eq!((over.row, over.col, over.time_s), (1, 0, Some(300.0)));
        assert_eq!(b.read_type, Some(crate::model::ReadType::Kinetic));
    }

    #[test]
    fn sparkcontrol_endpoint_list_and_kinetic_rows() {
        let text = "\u{feff}Method name: Method 1,,,,,,\n\
Application: SparkControl,,,,V2.3,,\n\
Device: Spark,,,,Serial number: 1806012423,,\n\
Date:,,,,19/12/2019,,\n\
Time:,,,,3:20 PM,,\n\
Plate,,,,[GRE96ft] - Greiner 96 Flat Transparent,,\n\
List of actions in this measurement script:,,,,,,\n\
Name,,,,GRE96ft,,\n\
Mode,Absorbance,,,,,\n\
Name,OD600,,,,,\n\
Measurement wavelength,,,,600,nm,\n\
Start Time,,,,19/12/2019 15:07,,\n\
Temperature,,,,28.5,°C,\n\
,,,,,,\n\
<>,Value,Time [ms],,,,\n\
A1,0.0921,0,,,,\n\
A2,0.0942,204,,,,\n\
End Time,,,,19/12/2019 15:08,,\n\
Mode,Fluorescence Top Reading,,,,,\n\
Name,Gain 40,,,,,\n\
Excitation wavelength,,,,485,nm,\n\
Emission wavelength,,,,535,nm,\n\
Gain,,,,40,Manual,\n\
<>,Value,Time [ms],,,,\n\
A1,120,0,,,,\n\
A2,,204,,,,\n";
        let ex = parse(text);
        assert_eq!(ex.software_version.as_deref(), Some("2.3"));
        let b = &ex.blocks[0];
        assert_eq!(b.channels.len(), 2);
        assert_eq!(b.channels[0].label, "OD600");
        assert_eq!(b.channels[1].label, "Gain 40");
        assert_eq!(b.channels[1].excitation_nm, Some(485.0));
        assert_eq!(b.channels[1].emission_nm, Some(535.0));
        assert_eq!(b.obs.len(), 3);
        assert_eq!(b.temperature_c, Some(28.5));
    }
}
