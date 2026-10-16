//! Sartorius Incucyte metric exports (Incucyte Zoom, S3, SX5 software: "Export Data" of one
//! metric). Two layouts (`docs/formats/plate-readers.md` § Incucyte):
//!
//! - **Wells as columns**: `Key: value` lines (`Vessel Name` or `Label`, `Metric`, `Cell Type`,
//!   `Passage`, `Notes`, `Analysis` or `Analysis Job`), a blank line, then `Date Time`,
//!   `Elapsed` (hours) and one column per well (`B2`, `C2`, ...), one row per scan.
//! - **Plate maps**: the same key lines, then per scan a `Time Stamp:` line with the date and
//!   `Elapsed:` hours, followed by a plate matrix (column numbers, row letters).
//!
//! Each scan is a kinetic time point of one read (the metric), so the growth and kinetics
//! analyses and the plate previews apply. Columns that are not wells (group means and their
//! `(Std Dev)` columns, per-image columns `A1, Image 1`) are not read; the file is refused when
//! no column is a well.

use serde_json::json;

use crate::datetime::combine;
use crate::grid::parse_well;
use crate::model::{Block, Channel, Export, Kind, Mode, ModeBasis, ReadType};
use crate::sheet::{Book, Sheet};

/// The key lines Incucyte writes above its data.
const KEYS: [&str; 8] = [
    "vessel name",
    "label",
    "metric",
    "cell type",
    "passage",
    "notes",
    "analysis",
    "analysis job",
];

/// `Key: value` of a line (its cells joined), when the key is one Incucyte writes.
fn key_line(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once(':')?;
    let key = k.trim();
    KEYS.contains(&key.to_ascii_lowercase().as_str())
        .then(|| (key.to_string(), v.trim().to_string()))
}

/// The head of a text file looks like an Incucyte metric export.
pub(crate) fn sniff(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().take(40).collect();
    let first = lines
        .iter()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let starts = first.starts_with("vessel name:") || first.starts_with("label:");
    let metric = lines
        .iter()
        .any(|l| l.trim().to_ascii_lowercase().starts_with("metric:"));
    let data = lines.iter().any(|l| {
        let l = l.trim().to_ascii_lowercase();
        l.starts_with("date time") && l.contains("elapsed") || l.starts_with("time stamp:")
    });
    starts && metric && data
}

/// A row's cells joined by tabs.
fn line_of(sheet: &Sheet, r: usize) -> String {
    sheet.row_texts(r).join("\t")
}

/// A row's cells in their positions (blank ones as empty text).
fn cells(sheet: &Sheet, r: usize) -> Vec<String> {
    (0..sheet.row_len(r)).map(|k| sheet.text(r, k)).collect()
}

/// The metric's unit from its name: `Phase Object Confluence (Percent)` → `%`.
fn unit_of(metric: &str) -> Option<String> {
    let open = metric.rfind('(')?;
    let inner = metric[open + 1..].trim_end_matches(')').trim();
    Some(match inner.to_ascii_lowercase().as_str() {
        "percent" | "%" => "%".to_string(),
        _ => inner.replace("mm^2", "mm²").replace("um", "µm"),
    })
    .filter(|u| !u.is_empty())
}

pub(crate) fn parse(book: &Book) -> Export {
    let mut ex = Export::new(Kind::Incucyte, book.container.clone());
    let Some(sheet) = book.sheets.first() else {
        return ex;
    };
    let mut r = 0;
    while r < sheet.rows.len() {
        let line = line_of(sheet, r);
        let l = line.trim().to_ascii_lowercase();
        if l.starts_with("date time") || l.starts_with("time stamp:") {
            break;
        }
        if let Some((k, v)) = key_line(&line) {
            ex.put(k, v);
        }
        r += 1;
    }
    ex.experiment = ex
        .get("Vessel Name")
        .or_else(|| ex.get("Label"))
        .map(str::to_string);
    ex.protocol = ex
        .get("Analysis")
        .or_else(|| ex.get("Analysis Job"))
        .map(str::to_string);
    let metric = ex.get("Metric").unwrap_or("metric").to_string();
    // the read is named without its unit: `Phase Object Confluence`, unit `%`
    let unit = unit_of(&metric);
    let name = match (unit.as_ref(), metric.rfind('(')) {
        (Some(_), Some(i)) if i > 0 => metric[..i].trim().to_string(),
        _ => metric.clone(),
    };
    let mut block = Block::new(name.clone(), ex.experiment.clone().unwrap_or_default());
    block.read_type = Some(ReadType::Kinetic);
    block.line = Some(r);
    let mut ch = Channel::derived(&name, Mode::Imaging, ModeBasis::Stated);
    ch.unit = unit;
    for (k, key) in [("cell_type", "Cell Type"), ("passage", "Passage")] {
        if let Some(v) = ex.get(key) {
            ch.settings.insert(k.into(), json!(v));
        }
    }
    let c = block.channel(ch);
    let start = r;
    if sheet
        .text(start, 0)
        .trim()
        .to_ascii_lowercase()
        .starts_with("time stamp")
    {
        plate_maps(sheet, start, &mut block, c, &mut ex);
    } else {
        well_columns(sheet, start, &mut block, c, &mut ex);
    }
    if !block.obs.is_empty() {
        ex.blocks.push(block);
    }
    ex
}

/// `Date Time`, `Elapsed`, wells.
fn well_columns(sheet: &Sheet, header: usize, block: &mut Block, c: u32, ex: &mut Export) {
    let titles = cells(sheet, header);
    let wells: Vec<(usize, (u32, u32))> = titles
        .iter()
        .enumerate()
        .skip(2)
        .filter_map(|(k, t)| parse_well(t).map(|w| (k, w)))
        .collect();
    let skipped: Vec<&str> = titles
        .iter()
        .skip(2)
        .filter(|t| !t.trim().is_empty() && parse_well(t).is_none())
        .map(String::as_str)
        .collect();
    if !skipped.is_empty() {
        ex.notes.push(format!(
            "{} column(s) are not wells and were not read (group means, standard deviations or per-image values), e.g. {:?}",
            skipped.len(),
            skipped[0]
        ));
        ex.sections
            .insert("columns_not_read".into(), json!(skipped));
    }
    let mut first_time: Option<String> = None;
    for r in header + 1..sheet.rows.len() {
        if sheet.row_is_blank(r) {
            continue;
        }
        let Some(hours) = crate::sheet::parse_number(&sheet.text(r, 1).replace(',', ".")) else {
            continue;
        };
        if first_time.is_none() {
            first_time = Some(sheet.text(r, 0));
        }
        let t = hours * 3600.0;
        block.decimal_comma = sheet.text(r, 1).contains(',');
        for &(k, (row, col)) in &wells {
            block.push_cell(row, col, c, Some(t), None, sheet.cell(r, k));
        }
    }
    if let Some(t) = first_time {
        ex.set_acquired(&t, None);
    }
}

/// `Time Stamp:` lines each followed by a plate matrix.
fn plate_maps(sheet: &Sheet, start: usize, block: &mut Block, c: u32, ex: &mut Export) {
    let mut r = start;
    let mut first = true;
    while r < sheet.rows.len() {
        let texts = sheet.row_texts(r);
        if !texts
            .first()
            .is_some_and(|s| s.trim().to_ascii_lowercase().starts_with("time stamp"))
        {
            r += 1;
            continue;
        }
        // `Time Stamp:` <date> `Elapsed:` <hours>, as cells or in one cell
        let joined = texts.join("\t");
        let after = joined.split_once(':').map_or("", |x| x.1);
        let (stamp, elapsed) = match after.to_ascii_lowercase().find("elapsed:") {
            Some(i) => (&after[..i], &after[i + "elapsed:".len()..]),
            None => (after, ""),
        };
        let stamp = stamp.trim().trim_matches('\t').trim();
        let hours = crate::sheet::parse_number(elapsed.trim().trim_matches('\t').trim());
        if first && !stamp.is_empty() {
            ex.set_acquired(stamp, None);
            first = false;
        }
        let t = hours.map(|h| h * 3600.0).or_else(|| {
            let t0 = ex.acquired_at.clone()?;
            let now = combine(stamp, None)?.0;
            seconds_between(&t0, &now)
        });
        // the matrix: a header of column numbers, then rows starting with a row letter
        let cols: Vec<Option<u32>> = cells(sheet, r + 1)
            .iter()
            .map(|s| s.trim().parse::<u32>().ok().filter(|n| *n >= 1))
            .collect();
        let mut rr = r + 2;
        while rr < sheet.rows.len() {
            let label = sheet.text(rr, 0);
            let Some(row) = crate::grid::parse_row_label(label.trim(), false) else {
                break;
            };
            for (k, col) in cols.iter().enumerate().skip(1) {
                if let Some(col) = col {
                    block.push_cell(row, col - 1, c, t, None, sheet.cell(rr, k));
                }
            }
            rr += 1;
        }
        r = rr;
    }
}

/// Seconds from ISO time `a` to `b` (both local `YYYY-MM-DDTHH:MM:SS`).
#[allow(clippy::many_single_char_names)] // y, m, d: the parts of a date
fn seconds_between(a: &str, b: &str) -> Option<f64> {
    let s = |t: &str| -> Option<f64> {
        let (d, c) = t.split_once('T')?;
        let mut p = d.split('-').map(|x| x.parse::<i64>().ok());
        let (y, m, dd) = (p.next()??, p.next()??, p.next()??);
        let secs = crate::datetime::parse_clock(c)?;
        // days from a civil date (Howard Hinnant's algorithm)
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + dd - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some((era * 146_097 + doe) as f64 * 86_400.0 + secs)
    };
    Some(s(b)? - s(a)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    const WELLS: &str = "Label: DJ_MDA\nMetric: Phase Object Confluence (Percent)\nCell Type: MDA-MB-468\nPassage: 1\nNotes: \nAnalysis Job: job\n\nDate Time\tElapsed\tB2\tC2\n30/01/2014 17:00:00\t0\t15.7\t13.9\n30/01/2014 21:00:00\t4\t16.6\t14.6\n";
    const MAPS: &str = "Vessel Name:\tplate_1\nMetric:\tSynthetic green area\nAnalysis:\tSynthetic example\nTime Stamp:\t01/01/2026 08:00:00 AM\tElapsed:\t0\n\t1\t2\nA\t10\t12\nB\t5\t6\nTime Stamp:\t01/01/2026 10:00:00 AM\tElapsed:\t2\n\t1\t2\nA\t20\t24\nB\t7\t8\n";
    const GROUPS: &str = "Vessel Name: X\nMetric: Phase Object Confluence (%)\n\nDate Time\tElapsed\tRef\tRef (Std Dev)\n01.01.2026 08:00:00\t0,0\t8,0\t0,4\n";

    #[test]
    fn wells_as_columns() {
        assert!(sniff(WELLS));
        let ex = parse(&text_book(WELLS.as_bytes()));
        assert_eq!(ex.experiment.as_deref(), Some("DJ_MDA"));
        let b = &ex.blocks[0];
        assert_eq!(b.channels[0].unit.as_deref(), Some("%"));
        assert_eq!(b.obs.len(), 4);
        assert_eq!(b.time_points(), vec![0.0, 4.0 * 3600.0]);
        assert_eq!(ex.acquired_at.as_deref(), Some("2014-01-30T17:00:00"));
    }

    #[test]
    fn plate_maps_layout() {
        assert!(sniff(MAPS));
        let ex = parse(&text_book(MAPS.as_bytes()));
        let b = &ex.blocks[0];
        assert_eq!(b.obs.len(), 8);
        assert_eq!(b.time_points(), vec![0.0, 7200.0]);
    }

    #[test]
    fn groups_only_are_not_wells() {
        assert!(sniff(GROUPS));
        let ex = parse(&text_book(GROUPS.as_bytes()));
        assert!(ex.blocks.is_empty());
        assert!(ex.notes[0].contains("not wells"));
    }

    #[test]
    fn days_between_dates() {
        assert_eq!(
            seconds_between("2026-01-01T08:00:00", "2026-01-02T08:00:00"),
            Some(86_400.0)
        );
    }

    #[test]
    fn truncations_do_not_panic() {
        for t in [WELLS, MAPS, GROUPS] {
            for cut in 0..t.len() {
                if let Some(s) = t.get(..cut) {
                    let _ = sniff(s);
                    let _ = parse(&text_book(s.as_bytes()));
                }
            }
        }
    }
}
