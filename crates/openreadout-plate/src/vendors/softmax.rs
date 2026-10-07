//! Molecular Devices SoftMax Pro text exports (`##BLOCKS= n`, blocks ending in `~End`).
//!
//! `Plate:` blocks carry a positional header line (docs/formats/plate-readers.md, "SoftMax
//! Pro"), then the raw data in PlateFormat (plate-shaped; several wavelengths side by side,
//! one empty column apart; kinetic snapshots one after another) or TimeFormat (wells as
//! columns, one line per time point or wavelength), optionally followed by reduced data.
//! `Group:` and `Note:` blocks are kept verbatim.

use openreadout_core::model::Finding;
use serde_json::json;

use crate::datetime::parse_duration;
use crate::grid::{column_number, parse_well};
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType, nonempty};
use crate::sheet::{Book, Sheet};

pub(crate) fn sniff(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("##BLOCKS=")
}

/// Fields of a `Plate:` header line.
#[derive(Debug, Clone, Default)]
struct Header {
    name: String,
    export_version: String,
    format: String,
    read_type: String,
    read_mode: String,
    data_type: String,
    kinetic_points: Option<u32>,
    read_time: Option<String>,
    read_interval: Option<String>,
    spectrum: Option<(f64, f64, f64)>,
    wavelengths: Vec<f64>,
    excitation: Vec<f64>,
    cutoff: Vec<f64>,
    wells: Option<u32>,
    first_row: u32,
    rows: Option<u32>,
    reads_per_well: Option<String>,
    pmt_gain: Option<String>,
    bottom_read: Option<bool>,
}

fn floats(s: &str) -> Vec<f64> {
    s.split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect()
}

fn parse_header(f: &[String]) -> Header {
    let g = |i: usize| f.get(i).map_or("", |s| s.trim());
    let mode = g(5);
    // Fluorescence (and its polarization / time-resolved variants) has one extra field
    // (top or bottom read) right after the read mode.
    let fl = Mode::from_text(mode) == Mode::Fluorescence || mode.contains("Time Resolved");
    let o = usize::from(fl);
    let mut h = Header {
        name: g(1).to_string(),
        export_version: g(2).to_string(),
        format: g(3).to_string(),
        read_type: g(4).to_string(),
        read_mode: mode.to_string(),
        data_type: g(6 + o).to_string(),
        kinetic_points: g(8 + o).parse().ok(),
        read_time: nonempty(g(9 + o)),
        read_interval: nonempty(g(10 + o)),
        wavelengths: floats(g(15 + o)),
        wells: g(18 + o).parse().ok(),
        first_row: 1,
        ..Default::default()
    };
    if fl {
        h.bottom_read = match g(6) {
            "TRUE" => Some(true),
            "FALSE" => Some(false),
            _ => None,
        };
    }
    if let (Ok(a), Ok(b), Ok(s)) = (
        g(11 + o).parse::<f64>(),
        g(12 + o).parse::<f64>(),
        g(13 + o).parse::<f64>(),
    ) && s > 0.0
        && b >= a
    {
        h.spectrum = Some((a, b, s));
    }
    if Mode::from_text(mode) == Mode::Absorbance {
        h.first_row = g(19).parse().unwrap_or(1).max(1);
        h.rows = g(20).parse().ok();
    } else {
        h.excitation = floats(g(19 + o));
        h.cutoff = floats(g(21 + o));
        h.reads_per_well = nonempty(g(24 + o));
        h.pmt_gain = nonempty(g(25 + o));
        h.first_row = g(28 + o).parse().unwrap_or(1).max(1);
        h.rows = g(29 + o).parse().ok();
    }
    h
}

pub(crate) fn parse(book: &Book) -> Export {
    let sheet = &book.sheets[0];
    let mut ex = Export::new(Kind::SoftMaxPro, book.container.clone());
    ex.sheets_read.push(sheet.name.clone());
    let n = sheet.rows.len();
    let first = (0..n).find(|&r| {
        sheet
            .text(r, 0)
            .trim_start_matches('\u{feff}')
            .starts_with("##BLOCKS=")
    });
    let Some(first) = first else {
        return ex;
    };
    let declared: Option<usize> = sheet
        .text(first, 0)
        .trim_start_matches('\u{feff}')
        .trim_start_matches("##BLOCKS=")
        .trim()
        .parse()
        .ok();
    ex.put(
        "##BLOCKS",
        declared.map_or_else(String::new, |d| d.to_string()),
    );
    let mut r = first + 1;
    let mut blocks_seen = 0usize;
    let mut notes = Vec::new();
    let mut groups = Vec::new();
    while r < n {
        if sheet.row_is_blank(r) {
            r += 1;
            continue;
        }
        let head = sheet.text(r, 0);
        let end = (r..n).find(|&e| sheet.text(e, 0) == "~End").unwrap_or(n);
        if head.starts_with("Original Filename") || head.contains("Date Last Saved") {
            let line = sheet.row_line(r);
            for part in line.split(';') {
                if let Some((k, v)) = part.split_once(':') {
                    ex.put(k.trim(), v.trim());
                }
            }
            r += 1;
            continue;
        }
        blocks_seen += 1;
        let lines: Vec<String> = (r..end).map(|i| sheet.row_line(i)).collect();
        if head.starts_with("Plate:") {
            let fields: Vec<String> = sheet.rows[r].iter().map(crate::sheet::Cell::text).collect();
            let h = parse_header(&fields);
            let block = plate_block(sheet, r, end, &h, blocks_seen);
            ex.blocks.push(block);
        } else if head.starts_with("Group:") {
            groups.push(json!(lines));
        } else {
            notes.push(json!(lines));
        }
        r = end + 1;
    }
    if !notes.is_empty() {
        ex.sections.insert("notes".into(), json!(notes));
    }
    if !groups.is_empty() {
        ex.sections.insert("groups".into(), json!(groups));
    }
    if let Some(d) = declared
        && d != blocks_seen
    {
        ex.findings.push(Finding::warning(
            "block_count_mismatch",
            format!("##BLOCKS= declares {d} blocks, {blocks_seen} found"),
        ));
    }
    // The footer's save time is the only time these exports state. It follows the read (a
    // kinetic run by at least its length), so it is reported as the save time, never as the
    // read's start.
    if let Some(saved) = ex.get("Date Last Saved").map(str::to_string) {
        ex.saved_raw = Some(saved.clone());
        if let Some((iso, assumed)) = crate::datetime::combine(&saved, None) {
            for b in &mut ex.blocks {
                b.extra.insert("saved_at".into(), json!(iso));
            }
            ex.date_order_assumed = assumed;
            ex.notes.push("SoftMax Pro text exports record when the file was saved (`saved_at`), not when the plate was read; no read time is reported".into());
        }
    }
    ex
}

fn mode_of(h: &Header) -> Mode {
    Mode::from_text(&h.read_mode)
}

fn channels(h: &Header, block: &mut Block) -> Vec<u32> {
    let mode = mode_of(h);
    let unit = match mode {
        Mode::Absorbance => "OD",
        Mode::Fluorescence => "RFU",
        Mode::Luminescence => "RLU",
        _ => "",
    };
    let settings = |c: &mut Channel| {
        if let Some(v) = &h.reads_per_well {
            c.settings.insert("reads_per_well".into(), json!(v));
        }
        if let Some(v) = &h.pmt_gain {
            c.settings.insert("pmt_gain".into(), json!(v));
        }
        if let Some(b) = h.bottom_read {
            c.settings
                .insert("optics".into(), json!(if b { "Bottom" } else { "Top" }));
        }
    };
    if h.read_type == "Spectrum" {
        let mut c = Channel::new(format!("{} spectrum", h.read_mode), mode);
        c.unit = nonempty(unit);
        settings(&mut c);
        return vec![block.channel(c)];
    }
    let n = h.wavelengths.len().max(1);
    (0..n)
        .map(|i| {
            let wl = h.wavelengths.get(i).copied();
            let mut c = Channel::new(
                wl.map_or_else(
                    || h.read_mode.clone(),
                    |w| format!("{} {}", h.read_mode, crate::sheet::fmt_num(w)),
                ),
                mode,
            );
            match mode {
                Mode::Fluorescence => {
                    c.emission_nm = wl;
                    c.excitation_nm = h.excitation.get(i).copied();
                    c.label = format!(
                        "{} ex {} em {}",
                        h.read_mode,
                        c.excitation_nm
                            .map_or_else(|| "?".into(), crate::sheet::fmt_num),
                        wl.map_or_else(|| "?".into(), crate::sheet::fmt_num)
                    );
                    if let Some(cut) = h.cutoff.get(i) {
                        c.settings.insert("cutoff_nm".into(), json!(cut));
                    }
                }
                Mode::Luminescence => {
                    c.emission_nm = wl.filter(|w| *w > 0.0);
                    if c.emission_nm.is_none() {
                        c.label.clone_from(&h.read_mode);
                    }
                }
                _ => c.wavelength_nm = wl,
            }
            if n > 1 {
                // wavelengths may repeat (`426 426 426`): keep each a separate read
                c.label = format!("{} (wavelength {} of {n})", c.label, i + 1);
            }
            c.unit = nonempty(unit);
            settings(&mut c);
            block.channel(c)
        })
        .collect()
}

fn plate_block(sheet: &Sheet, start: usize, end: usize, h: &Header, index: usize) -> Block {
    let mut b = Block::new(
        if h.name.is_empty() {
            format!("Plate block {index}")
        } else {
            h.name.clone()
        },
        h.name.clone(),
    );
    b.line = Some(start + 1);
    b.decimal_comma = true;
    b.declared_wells = h.wells;
    b.read_type = Some(match h.read_type.as_str() {
        "Kinetic" => ReadType::Kinetic,
        "Spectrum" => ReadType::Spectrum,
        _ => ReadType::Endpoint,
    });
    for (k, v) in [
        ("export_version", Some(h.export_version.clone())),
        ("export_format", Some(h.format.clone())),
        ("softmax_read_type", Some(h.read_type.clone())),
        ("softmax_read_mode", Some(h.read_mode.clone())),
        ("data_type", Some(h.data_type.clone())),
        ("read_time", h.read_time.clone()),
        ("read_interval", h.read_interval.clone()),
    ] {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            b.extra.insert(k.into(), json!(v));
        }
    }
    if let Some(k) = h.kinetic_points {
        b.extra.insert("kinetic_points".into(), json!(k));
    }
    if h.export_version != "1.3" {
        b.findings.push(Finding::info(
            "export_version",
            format!(
                "{}: export version {} (1.3 is the one documented)",
                b.name, h.export_version
            ),
        ));
    }
    if !matches!(h.read_type.as_str(), "Endpoint" | "Kinetic" | "Spectrum") {
        b.findings.push(Finding::warning(
            "unsupported_read_type",
            format!("{}: read type {:?} is not decoded", b.name, h.read_type),
        ));
        return b;
    }
    let chans = channels(h, &mut b);
    if h.format == "TimeFormat" {
        time_format(sheet, start + 1, end, h, &chans, &mut b);
    } else {
        plate_format(sheet, start + 1, end, h, &chans, &mut b);
    }
    if let Some((a, z, s)) = h.spectrum.filter(|_| h.read_type == "Spectrum") {
        b.extra.insert(
            "spectrum_nm".into(),
            json!({"start": a, "end": z, "step": s}),
        );
    }
    b
}

fn spectrum_wavelengths(h: &Header) -> Vec<f64> {
    match h.spectrum {
        Some((start, end, step)) => {
            let count = ((end - start) / step).round() as usize + 1;
            (0..count.min(100_000))
                .map(|i| start + step * i as f64)
                .collect()
        }
        None => Vec::new(),
    }
}

/// Column groups of a PlateFormat header: runs of column numbers separated by a blank cell.
fn groups(sheet: &Sheet, r: usize) -> Vec<Vec<(usize, u32)>> {
    let width = sheet.rows.get(r).map_or(0, Vec::len);
    let mut out: Vec<Vec<(usize, u32)>> = Vec::new();
    let mut cur: Vec<(usize, u32)> = Vec::new();
    for c in 2..width {
        match column_number(sheet.cell(r, c)) {
            Some(n) => cur.push((c, n)),
            None => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn plate_format(sheet: &Sheet, start: usize, end: usize, h: &Header, chans: &[u32], b: &mut Block) {
    let Some(header) = (start..end).find(|&r| !sheet.row_is_blank(r)) else {
        return;
    };
    let cols = groups(sheet, header);
    let rows = h
        .rows
        .or_else(|| h.wells.and_then(crate::grid::dims_for_wells).map(|d| d.0))
        .unwrap_or(8)
        // A header declaring 0 rows would never advance `r` below.
        .max(1) as usize;
    let spectral = h.read_type == "Spectrum";
    let spec_wl = spectrum_wavelengths(h);
    let mut r = header + 1;
    let mut snapshot = 0usize;
    let mut temps = Vec::new();
    while r < end {
        if sheet.row_is_blank(r) {
            r += 1;
            continue;
        }
        // Reduced data: a new header line (column numbers after two empty cells).
        if column_number(sheet.cell(r, 2)).is_some()
            && sheet.text(r, 1).is_empty()
            && sheet.text(r, 0).is_empty()
        {
            reduced_plate(sheet, r, end, rows, h, b);
            return;
        }
        let key = sheet.text(r, 0);
        let (time, wl) = if spectral {
            (
                None,
                crate::sheet::parse_number(&key).or_else(|| spec_wl.get(snapshot).copied()),
            )
        } else if h.read_type == "Kinetic" {
            (parse_duration(&key), None)
        } else {
            (None, None)
        };
        if let Some(t) = sheet.cell(r, 1).number() {
            temps.push(t);
        }
        for i in 0..rows {
            let rr = r + i;
            if rr >= end || (i > 0 && sheet.row_is_blank(rr)) {
                break;
            }
            let prow = (h.first_row as usize - 1 + i) as u32;
            for (g, group) in cols.iter().enumerate() {
                let ch = if spectral {
                    chans[0]
                } else {
                    chans.get(g).copied().unwrap_or(chans[0])
                };
                for &(c, n) in group {
                    let cell = sheet.cell(rr, c).clone();
                    b.push_cell(prow, n - 1, ch, time, wl, &cell);
                }
            }
        }
        snapshot += 1;
        r += rows;
    }
    finish_temps(b, temps);
}

fn finish_temps(b: &mut Block, temps: Vec<f64>) {
    if let Some(t) = temps.first() {
        b.temperature_c = Some(*t);
    }
    if temps.len() > 1 {
        b.extra.insert("temperatures_c".into(), json!(temps));
    }
}

fn reduced_plate(sheet: &Sheet, header: usize, end: usize, rows: usize, h: &Header, b: &mut Block) {
    let cols = groups(sheet, header);
    let mut c = Channel::new("Reduced", mode_of(h));
    c.calculated = true;
    let ch = b.channel(c);
    for i in 0..rows {
        let rr = header + 1 + i;
        if rr >= end || sheet.row_is_blank(rr) {
            break;
        }
        for group in cols.iter().take(1) {
            for &(c, n) in group {
                let cell = sheet.cell(rr, c).clone();
                b.push_cell(
                    (h.first_row as usize - 1 + i) as u32,
                    n - 1,
                    ch,
                    None,
                    None,
                    &cell,
                );
            }
        }
    }
}

fn time_format(sheet: &Sheet, start: usize, end: usize, h: &Header, chans: &[u32], b: &mut Block) {
    let Some(header) = (start..end).find(|&r| !sheet.row_is_blank(r)) else {
        return;
    };
    let wells: Vec<(usize, (u32, u32))> = (2..sheet.rows.get(header).map_or(0, Vec::len))
        .filter_map(|c| parse_well(&sheet.text(header, c)).map(|w| (c, w)))
        .collect();
    let spectral = h.read_type == "Spectrum";
    let spec_wl = spectrum_wavelengths(h);
    let mut r = header + 1;
    let mut group = 0usize;
    let mut line_in_group = 0usize;
    let mut temps = Vec::new();
    while r < end {
        if sheet.row_is_blank(r) {
            if line_in_group > 0 && !spectral {
                group += 1;
                line_in_group = 0;
            }
            r += 1;
            continue;
        }
        // Reduced data: a second well header with an empty temperature column.
        if sheet.text(r, 0).is_empty()
            && sheet.text(r, 1).is_empty()
            && parse_well(&sheet.text(r, 2)).is_some()
        {
            let mut c = Channel::new("Reduced", mode_of(h));
            c.calculated = true;
            let ch = b.channel(c);
            let red: Vec<(usize, (u32, u32))> = (2..sheet.rows[r].len())
                .filter_map(|c| parse_well(&sheet.text(r, c)).map(|w| (c, w)))
                .collect();
            if r + 1 < end {
                for &(c, (pr, pc)) in &red {
                    let cell = sheet.cell(r + 1, c).clone();
                    b.push_cell(pr, pc, ch, None, None, &cell);
                }
            }
            break;
        }
        let key = sheet.text(r, 0);
        let (time, wl, ch) = if spectral {
            (
                None,
                crate::sheet::parse_number(&key).or_else(|| spec_wl.get(line_in_group).copied()),
                chans[0],
            )
        } else {
            (
                parse_duration(&key),
                None,
                chans.get(group).copied().unwrap_or(chans[0]),
            )
        };
        if let Some(t) = sheet.cell(r, 1).number() {
            temps.push(t);
        }
        for &(c, (pr, pc)) in &wells {
            let cell = sheet.cell(r, c).clone();
            b.push_cell(
                pr,
                pc,
                ch,
                if h.read_type == "Kinetic" { time } else { None },
                wl,
                &cell,
            );
        }
        line_in_group += 1;
        r += 1;
    }
    finish_temps(b, temps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn plate_format_two_wavelengths() {
        let t = "##BLOCKS= 1\nPlate:\tP1\t1.3\tPlateFormat\tEndpoint\tAbsorbance\tRaw\tFALSE\t1\t\t\t\t\t\t2\t450 600 \t1\t2\t6\t1\t2\n\tTemperature(\u{a1}C)\t1\t2\t3\t\t1\t2\t3\n\t25\t0.1\t0.2\t0.3\t\t1.1\t1.2\tRange?\n\t\t0.4\t0.5\t0.6\t\t1.4\t1.5\t1.6\n\n~End\nOriginal Filename: x; Date Last Saved: 1/9/2023 2:23:07 PM\n";
        assert!(sniff(t));
        let ex = parse(&text_book(t.as_bytes()));
        let b = &ex.blocks[0];
        assert_eq!(b.channels.len(), 2);
        assert_eq!(b.channels[1].wavelength_nm, Some(600.0));
        assert_eq!(b.obs.len(), 12);
        assert_eq!(b.temperature_c, Some(25.0));
        assert!(b.obs.iter().any(|o| o.text.as_deref() == Some("Range?")));
        assert_eq!(ex.acquired_at, None, "the save time is not the read time");
        assert_eq!(ex.blocks[0].extra["saved_at"], "2023-01-09T14:23:07");
    }

    #[test]
    fn time_format_luminescence() {
        let t = "##BLOCKS= 1\nPlate:\tc1\t1.3\tTimeFormat\tEndpoint\tLuminescence\tRaw\tFALSE\t1\t\t\t\t\t\t1\t0 \t1\t12\t96\t\t\t\t\t\t6\t\t\t\t1\t4\n\tTemperature(C)\tA1\tA2\n\t25\t10\t20\n\n\t\tA1\tA2\n\t\t1\t2\n~End\n";
        let ex = parse(&text_book(t.as_bytes()));
        let b = &ex.blocks[0];
        assert_eq!(
            b.obs
                .iter()
                .filter(|o| !b.channels[o.channel as usize].calculated)
                .count(),
            2
        );
        assert_eq!(b.channels[0].mode, Mode::Luminescence);
        assert_eq!(b.channels[0].emission_nm, None);
        assert!(b.channels[1].calculated);
    }
}
