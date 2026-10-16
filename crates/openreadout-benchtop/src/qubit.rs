//! Thermo Fisher Qubit fluorometer exports (`docs/formats/qubit.md`):
//!
//! - `qubit-4`: Qubit 4 (and 3) CSV, header `Run ID,Assay Name,Test Name,Test Date,Qubit® tube
//!   conc.,Units,Original sample conc.,Units,...`, one row per tube read.
//! - `qubit-flex`: Qubit Flex CSV, header `Run ID,Test Date,Assay Name,Sample Name,Original
//!   Sample Conc.,Original sample conc. units,...`, optionally after a `sep=,` line.

use openreadout_core::{Error, Result};

use crate::model::Parsed;
use crate::table::{Role, build, key};
use crate::text::split;

pub(crate) const FORMAT_ID: &str = "qubit";
/// Largest export read.
pub(crate) const MAX_BYTES: u64 = 64 << 20;
/// Most rows accepted.
const MAX_ROWS: usize = 1_000_000;

/// The header line of a head, after an optional `sep=,` line.
fn header_line(text: &str) -> Option<(usize, &str)> {
    let mut it = text.lines().enumerate();
    let (i, first) = it.next()?;
    let first = first.trim_start_matches('\u{feff}').trim_end_matches('\r');
    if key(first).starts_with("sep=") {
        let (j, l) = it.next()?;
        return Some((j, l.trim_end_matches('\r')));
    }
    Some((i, first))
}

/// Which layout the head of a file is, if a Qubit one.
pub(crate) fn sniff(text: &str) -> Option<&'static str> {
    let (_, h) = header_line(text)?;
    let k = key(h);
    if k.starts_with("run id,assay name,test name,test date,") && k.contains("tube conc") {
        return Some("qubit-4");
    }
    if k.starts_with("run id,test date,assay name,sample name,") && k.contains("qubit tube conc") {
        return Some("qubit-flex");
    }
    None
}

fn role(k: &str) -> Option<Role> {
    use Role::{Concentration as C, Date, Text as T, Units, Value as V};
    Some(match k {
        "run id" => T("run_id"),
        "assay name" => T("assay"),
        "test name" | "sample name" => T("sample"),
        "test date" => Date("measured_at"),
        "last read standards" => Date("standards_read_at"),
        "qubit® tube conc." | "qubit tube conc." | "qubit tube conc" => {
            C("tube_concentration", None)
        }
        "original sample conc." | "original sample conc" => C("sample_concentration", None),
        "units" | "original sample conc. units" | "qubit tube conc. units" => Units,
        "sample volume (µl)" | "sample volume (ul)" => V("sample_volume", Some("µL")),
        "dilution factor" => V("dilution_factor", None),
        "std 1 rfu" => V("std1_rfu", Some("RFU")),
        "std 2 rfu" => V("std2_rfu", Some("RFU")),
        "std 3 rfu" => V("std3_rfu", Some("RFU")),
        "excitation" => T("excitation"),
        "emission" => T("emission"),
        "green rfu" => V("green_rfu", Some("RFU")),
        "far red rfu" => V("far_red_rfu", Some("RFU")),
        "sample rfu" => V("sample_rfu", Some("RFU")),
        "extended low range" => T("extended_low_range"),
        "core range" => T("core_range"),
        "extended high range" => T("extended_high_range"),
        "reagent lot#" => T("reagent_lot"),
        "plate barcode" => T("plate_barcode"),
        "well" => T("well"),
        "sample id" => T("sample_id"),
        "tags" => T("tags"),
        "calibrated tubes" => V("calibrated_tubes", None),
        "software version" => T("software_version"),
        "selected samples" => V("selected_samples", None),
        _ => return None,
    })
}

/// Parse a whole export.
pub(crate) fn parse(text: &str) -> Result<Parsed> {
    let layout = sniff(text)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "not a Qubit export layout this reader knows"))?;
    let (hi, h) = header_line(text).ok_or_else(|| Error::corrupt(FORMAT_ID, "empty file"))?;
    let mut headers = split(h, ',');
    while headers.last().is_some_and(|x| x.trim().is_empty()) {
        headers.pop();
    }
    let mut p = Parsed {
        layout,
        model: Some(
            if layout == "qubit-4" {
                "Qubit 4"
            } else {
                "Qubit Flex"
            }
            .into(),
        ),
        ..Parsed::default()
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    for l in text.lines().skip(hi + 1) {
        let l = l.trim_end_matches('\r');
        if l.trim().is_empty() {
            continue;
        }
        rows.push(split(l, ','));
        if rows.len() > MAX_ROWS {
            return Err(Error::corrupt(FORMAT_ID, "more rows than any export holds"));
        }
    }
    // Qubit Flex 1.5 writes the last seven columns without titles
    let widest = rows.iter().map(Vec::len).max().unwrap_or(0);
    if layout == "qubit-flex" && widest > headers.len() {
        let tail = [
            "Reagent Lot#",
            "Plate Barcode",
            "Well",
            "Sample ID",
            "Tags",
            "Calibrated Tubes",
            "Software Version",
            "Selected Samples",
        ];
        let have: Vec<String> = headers.iter().map(|h| key(h)).collect();
        for t in tail {
            if headers.len() >= widest {
                break;
            }
            if !have.contains(&key(t)) {
                headers.push(t.to_string());
            }
        }
        if headers.len() >= widest {
            p.notes.push(
                "the export's header names fewer columns than its rows hold; the untitled ones are read as the Qubit Flex's documented trailing columns (Plate Barcode ... Selected Samples)".into(),
            );
        }
    }
    // Qubit 4: the run id is the date of the run; it settles the day and month order
    let run_dates: Vec<Option<String>> = rows
        .iter()
        .map(|r| {
            r.first()
                .map(|x| x.trim().to_string())
                .filter(|x| x.len() == 10 && x.as_bytes()[4] == b'-' && x.as_bytes()[7] == b'-')
        })
        .collect();
    let hint = |i: usize| run_dates.get(i).cloned().flatten();
    let month_first = layout == "qubit-4";
    p.columns = build(
        &headers,
        &rows,
        &role,
        month_first,
        Some(&hint),
        &mut p.findings,
    );
    p.rows = rows.len();
    p.application = p.first_text("assay").map(str::to_string);
    p.software_version = p.first_text("software_version").map(str::to_string);
    Ok(p)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;
    use crate::model::Column;

    const Q4: &str = "Run ID,Assay Name,Test Name,Test Date,Qubit® tube conc.,Units,Original sample conc.,Units,Sample Volume (µL),Dilution Factor,Std 1 RFU,Std 2 RFU,Std 3 RFU,Excitation,Emission,Green RFU,Far Red RFU\n2023-11-04,1x dsDNA High Sensitivity,S1,04/11/2023 1:06:59 AM,106,ng/mL,21.2,ng/µL,1,200,56.12,26516.64,,Blue,Green,5615.1,\n";
    const FLEX: &str = "sep=,\nRun ID,Test Date,Assay Name,Sample Name,Original Sample Conc.,Original sample conc. units,Qubit Tube Conc.,Qubit tube conc. units,Sample Volume (uL),Dilution Factor,Extended Low Range,Core Range,Extended High Range,Excitation,Std 1 RFU,Std 2 RFU,Std 3 RFU,Sample RFU,Last Read Standards,Reagent Lot#,\n131222-164034,13/12/2022 04:41:09 PM,RNA BR,S1,528,ng/uL,2.64,ug/mL,1,200,10.0-20.0,20.0-1000,1000-1200,RED,3.19,45.42,,26.8,13/12/2022 04:40:57 PM,,,,,,6,1.5.0,6\n";

    #[test]
    fn qubit4() {
        let p = parse(Q4).unwrap();
        assert_eq!(p.layout, "qubit-4");
        assert_eq!(p.first_text("measured_at"), Some("2023-11-04T01:06:59"));
        let Some(Column::Numbers { values, unit, .. }) = p.column("tube_concentration") else {
            panic!()
        };
        assert_eq!((values[0], unit.as_deref()), (106.0, Some("ng/mL")));
        let Some(Column::Numbers { unit, .. }) = p.column("sample_concentration") else {
            panic!()
        };
        assert_eq!(unit.as_deref(), Some("ng/µL"));
        assert_eq!(p.application.as_deref(), Some("1x dsDNA High Sensitivity"));
    }

    #[test]
    fn flex() {
        let p = parse(FLEX).unwrap();
        assert_eq!(p.layout, "qubit-flex");
        assert_eq!(p.software_version.as_deref(), Some("1.5.0"));
        let Some(Column::Numbers { unit, .. }) = p.column("tube_concentration") else {
            panic!()
        };
        assert_eq!(unit.as_deref(), Some("µg/mL"));
        assert_eq!(p.first_text("measured_at"), Some("2022-12-13T16:41:09"));
    }

    #[test]
    fn truncations_do_not_panic() {
        for t in [Q4, FLEX] {
            for cut in 0..t.len() {
                if let Some(s) = t.get(..cut) {
                    let _ = parse(s);
                }
            }
        }
    }
}
