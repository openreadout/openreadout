//! Bio-Rad droplet digital PCR exports (`docs/formats/biorad-ddpcr.md`): the CSV files that
//! QuantaSoft and QX Manager write for a QX100/QX200/QX600 plate.
//!
//! - `quantasoft-results`: one row per well and target, header `Well,ExptType,Experiment,
//!   Sample,TargetType,Target,Status,Concentration,...` (older versions write `TypeAssay,Assay`
//!   for `TargetType,Target`). Some versions write more values per row than the header names;
//!   those cells are left out and counted in a note.
//! - `qx-manager-results`: one row per well and target, header `Well,Sample description 1,...,
//!   Target,Conc(copies/µL),...,Accepted Droplets,Positives,Negatives,...`.
//! - `quantasoft-amplitude`: one row per droplet of one well, `Ch1 Amplitude,Ch2 Amplitude,
//!   Cluster` (or `Assay1 Amplitude,...`).
//! - `qx-manager-amplitude`: one row per droplet of one well, after `Target Value of ...` lines:
//!   `Ch1Amplitude,Ch2Amplitude,<target>,<target>,...`, each target column `0` (negative), `1`
//!   (positive) or `u` (unclassified).

use openreadout_core::model::Finding;
use openreadout_core::{Error, Result};

use crate::model::{Column, Parsed, numbers, slug};
use crate::table::{Role, build, key};
use crate::text::{number, split};

pub(crate) const FORMAT_ID: &str = "biorad-ddpcr";
/// Largest export read (a well's amplitude file is a few MB; a 96-well results table far less).
pub(crate) const MAX_BYTES: u64 = 256 << 20;
/// Most rows accepted (droplets in a well, or well × target rows).
const MAX_ROWS: usize = 4_000_000;

/// The header cells of a results export, normalised.
fn header_keys(line: &str) -> Vec<String> {
    split(line.trim_start_matches('\u{feff}').trim_end_matches('\r'), ',')
        .iter()
        .map(|h| key(h))
        .collect()
}

/// The first line and the line index of the header of an amplitude export (after any
/// `Target Value of ...` lines and blank lines).
fn amplitude_header(text: &str) -> Option<(usize, &str)> {
    for (i, l) in text.lines().enumerate().take(16) {
        let t = l.trim_start_matches('\u{feff}').trim();
        if t.is_empty() || key(t).starts_with("target value of") {
            continue;
        }
        let k = key(t);
        let first = k.split(',').next().unwrap_or_default();
        let amp = first.replace(' ', "");
        if (amp.starts_with("ch1amplitude") || amp.starts_with("assay1amplitude"))
            && k.split(',').nth(1).is_some_and(|c| c.replace(' ', "").ends_with("amplitude"))
        {
            return Some((i, l.trim_end_matches('\r')));
        }
        return None;
    }
    None
}

/// Which layout the head of a file is, if a ddPCR export.
pub(crate) fn sniff(text: &str) -> Option<&'static str> {
    if let Some((i, _)) = amplitude_header(text) {
        return Some(if i == 0 {
            "quantasoft-amplitude"
        } else {
            "qx-manager-amplitude"
        });
    }
    let first = text.lines().next()?;
    let k = header_keys(first);
    if k.first().map(String::as_str) != Some("well") {
        return None;
    }
    let has = |h: &str| k.iter().any(|c| c == h);
    if has("accepteddroplets") && has("positives") && (has("concentration") || has("conc")) {
        return Some("quantasoft-results");
    }
    if has("accepted droplets") && has("positives") && k.iter().any(|c| c.starts_with("conc(")) {
        return Some("qx-manager-results");
    }
    None
}

/// What a results header means (normalised headers of both results layouts).
fn role(k: &str) -> Option<Role> {
    use Role::{Concentration as C, Text as T, Value as V};
    const CPUL: Option<&str> = Some("copies/µL");
    Some(match k {
        "well" => T("well"),
        "expttype" => T("experiment_type"),
        "experiment" => T("experiment"),
        "sample" | "sample description 1" => T("sample"),
        "sample description 2" => T("sample_description_2"),
        "sample description 3" => T("sample_description_3"),
        "sample description 4" => T("sample_description_4"),
        "sampletype" => T("sample_type"),
        "targettype" | "typeassay" => T("target_type"),
        "target" | "assay" => T("target"),
        "status" => T("status"),
        "status reason" => T("status_reason"),
        "supermix" => T("supermix"),
        "dyename(s)" => T("dye"),
        "concentration" | "conc(copies/µl)" => C("concentration", CPUL),
        "pg/µl" => V("mass_concentration", Some("pg/µL")),
        "copiesper20ulwell" | "copies/20µlwell" => V("copies_per_20ul_well", Some("copies")),
        "totalconfmax" => V("total_ci95_max", CPUL),
        "totalconfmin" => V("total_ci95_min", CPUL),
        "poissonconfmax" => V("poisson_ci95_max", CPUL),
        "poissonconfmin" => V("poisson_ci95_min", CPUL),
        "totalconfidencemax68" => V("total_ci68_max", CPUL),
        "totalconfidencemin68" => V("total_ci68_min", CPUL),
        "poissonconfidencemax68" => V("poisson_ci68_max", CPUL),
        "poissonconfidencemin68" => V("poisson_ci68_min", CPUL),
        "positives" => V("positive_droplets", Some("droplets")),
        "negatives" => V("negative_droplets", Some("droplets")),
        "accepteddroplets" | "accepted droplets" => V("accepted_droplets", Some("droplets")),
        "ch1+ch2+" => V("droplets_ch1_pos_ch2_pos", Some("droplets")),
        "ch1+ch2-" => V("droplets_ch1_pos_ch2_neg", Some("droplets")),
        "ch1-ch2+" => V("droplets_ch1_neg_ch2_pos", Some("droplets")),
        "ch1-ch2-" => V("droplets_ch1_neg_ch2_neg", Some("droplets")),
        "linkage" | "copies/ul linked molecules" => V("linked_molecules", CPUL),
        "cnv" => V("copy_number", None),
        "referencecopies" => V("reference_copies", None),
        "unknowncopies" => V("unknown_copies", None),
        "ratio" => V("ratio", None),
        "fractionalabundance" | "fractional abundance" => {
            V("fractional_abundance", Some("%"))
        }
        "meanamplitudeofpositives" => V("mean_amplitude_positives", None),
        "meanamplitudeofnegatives" => V("mean_amplitude_negatives", None),
        "meanamplitudetotal" => V("mean_amplitude", None),
        "threshold1" | "threshold" => V("threshold", None),
        "experimentcomments" => T("experiment_comments"),
        "mergedwells" => T("merged_wells"),
        "referenceused" => T("reference_used"),
        "tiltcorrected" => T("tilt_corrected"),
        _ => return None,
    })
}

/// Parse a whole export. `file_name` gives the well of an amplitude file (`..._A01_Amplitude.csv`).
pub(crate) fn parse(text: &str, file_name: &str) -> Result<Parsed> {
    let layout = sniff(text).ok_or_else(|| {
        Error::corrupt(FORMAT_ID, "not a QuantaSoft or QX Manager export layout this reader knows")
    })?;
    if layout.ends_with("amplitude") {
        parse_amplitude(text, layout, file_name)
    } else {
        parse_results(text, layout)
    }
}

fn data_rows(lines: impl Iterator<Item = String>) -> Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    for l in lines {
        if l.trim().is_empty() || l.trim().chars().all(|c| c == ',') {
            continue;
        }
        rows.push(split(&l, ','));
        if rows.len() > MAX_ROWS {
            return Err(Error::corrupt(FORMAT_ID, "more rows than any export holds"));
        }
    }
    Ok(rows)
}

fn parse_results(text: &str, layout: &'static str) -> Result<Parsed> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r').to_string());
    let h = lines
        .next()
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "empty file"))?;
    let mut headers = split(h.trim_start_matches('\u{feff}'), ',');
    while headers.last().is_some_and(|x| x.trim().is_empty()) {
        headers.pop();
    }
    let mut rows = data_rows(lines)?;
    // values past the last titled column (QuantaSoft 1.7 writes about 35 of them)
    let mut untitled = 0usize;
    for r in &mut rows {
        if r.len() > headers.len() {
            untitled += r[headers.len()..]
                .iter()
                .filter(|c| !c.trim().is_empty())
                .count();
            r.truncate(headers.len());
        }
    }
    let mut p = Parsed {
        layout,
        software: Some(
            if layout == "quantasoft-results" {
                "QuantaSoft"
            } else {
                "QX Manager"
            }
            .into(),
        ),
        table_name: "results",
        table_kind: "well_targets",
        ..Parsed::default()
    };
    p.columns = build(&headers, &rows, &role, true, None, &mut p.findings);
    p.rows = rows.len();
    if untitled > 0 {
        p.notes.push(format!(
            "{untitled} cell(s) past the last titled column are left out (the export gives them no header)"
        ));
    }
    p.notes
        .push("table 0 has one row per well and target, as the software exported it".into());
    Ok(p)
}

/// The well in an amplitude file's name: `quant_A01_Amplitude.csv` -> `A01`.
fn well_of(file_name: &str) -> Option<String> {
    let stem = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let lower = stem.to_ascii_lowercase();
    let i = lower.rfind("_amplitude")?;
    let before = &stem[..i];
    let w = before.rsplit('_').next()?;
    let mut ch = w.chars();
    let row = ch.next()?;
    let digits: String = ch.collect();
    (row.is_ascii_alphabetic() && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| format!("{}{digits}", row.to_ascii_uppercase()))
}

fn parse_amplitude(text: &str, layout: &'static str, file_name: &str) -> Result<Parsed> {
    let (hi, h) = amplitude_header(text)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no amplitude header line"))?;
    let mut headers = split(h.trim_start_matches('\u{feff}'), ',');
    while headers.last().is_some_and(|x| x.trim().is_empty()) {
        headers.pop();
    }
    let preamble: Vec<String> = text
        .lines()
        .take(hi)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let rows = data_rows(
        text.lines()
            .skip(hi + 1)
            .map(|l| l.trim_end_matches('\r').to_string()),
    )?;
    let mut findings = Vec::new();
    let mut columns: Vec<Column> = Vec::new();
    let mut unclassified = 0usize;
    let mut bad = 0usize;
    for (k, label) in headers.iter().enumerate() {
        let label = label.trim();
        let compact = key(label).replace(' ', "");
        let cells = rows.iter().map(|r| r.get(k).map_or("", |c| c.trim()));
        let (name, unit) = if let Some(ch) = compact
            .strip_suffix("amplitude")
            .and_then(|s| s.strip_prefix("ch").or_else(|| s.strip_prefix("assay")))
        {
            (format!("ch{ch}_amplitude"), None)
        } else if compact == "cluster" {
            ("cluster".to_string(), None)
        } else {
            // a target column of a QX Manager export: 0 negative, 1 positive, u unclassified
            let values = cells
                .map(|c| match c {
                    "1" => 1.0,
                    "0" => 0.0,
                    "u" | "U" => {
                        unclassified += 1;
                        f64::NAN
                    }
                    _ => {
                        bad += 1;
                        f64::NAN
                    }
                })
                .collect();
            columns.push(numbers(
                &format!("target_{}", slug(label)),
                label,
                None,
                values,
            ));
            continue;
        };
        let values = cells
            .map(|c| {
                number(c).unwrap_or_else(|| {
                    if !c.is_empty() {
                        bad += 1;
                    }
                    f64::NAN
                })
            })
            .collect();
        columns.push(numbers(&name, label, unit, values));
    }
    if bad > 0 {
        findings.push(Finding::warning(
            "cells_not_numbers",
            format!("{bad} cell(s) are not numbers; they are NaN"),
        ));
    }
    let mut p = Parsed {
        layout,
        software: Some(
            if layout == "quantasoft-amplitude" {
                "QuantaSoft"
            } else {
                "QX Manager"
            }
            .into(),
        ),
        table_name: "droplets",
        table_kind: "droplets",
        well: well_of(file_name),
        columns,
        rows: rows.len(),
        findings,
        ..Parsed::default()
    };
    p.header = preamble
        .iter()
        .enumerate()
        .map(|(i, l)| (format!("line {}", i + 1), l.clone()))
        .collect();
    p.notes.push(format!(
        "table 0 has one row per droplet{}; amplitudes are the reader's fluorescence units",
        p.well.as_deref().map(|w| format!(" of well {w}")).unwrap_or_default()
    ));
    if columns_have_targets(&p.columns) {
        p.notes.push(
            "`target_<name>` columns hold each droplet's call for that target: 1 positive, 0 negative, NaN unclassified".into(),
        );
    }
    if unclassified > 0 {
        p.notes.push(format!(
            "{unclassified} target call(s) are `u` (unclassified), read as NaN"
        ));
    }
    Ok(p)
}

fn columns_have_targets(columns: &[Column]) -> bool {
    columns.iter().any(|c| c.name().starts_with("target_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(
            sniff("Well,ExptType,Experiment,Sample,TargetType,Target,Status,Concentration,Supermix,CopiesPer20uLWell,Positives,Negatives,AcceptedDroplets\n"),
            Some("quantasoft-results")
        );
        assert_eq!(
            sniff("Well,Sample description 1,Target,Conc(copies/µL),Accepted Droplets,Positives,Negatives\n"),
            Some("qx-manager-results")
        );
        assert_eq!(sniff("Ch1 Amplitude,Ch2 Amplitude,Cluster\n1,2,1\n"), Some("quantasoft-amplitude"));
        assert_eq!(
            sniff("Target Value of 0 = negative\nTarget Value of 1 = positive\n\nCh1Amplitude,Ch2Amplitude,1,2,\n1,2,0,1,\n"),
            Some("qx-manager-amplitude")
        );
        assert_eq!(sniff("Well,Sample,Cq\n"), None);
    }

    #[test]
    fn results_with_untitled_cells_and_no_call() {
        let t = "Well,ExptType,Experiment,Sample,TargetType,Target,Status,Concentration,Positives,Negatives,AcceptedDroplets\n\"M01\",Absolute Quantification,\"ABS\",\"S1\",Ch1Unknown,\"ZFY\",,7.9,200,29859,30059,,0.18,1\n\"M02\",Absolute Quantification,\"ABS\",\"NTC\",Ch1Unknown,\"ZFY\",,No Call,0,75618,75618\n";
        let p = parse(t, "x.csv").unwrap();
        assert_eq!(p.rows, 2);
        let Some(Column::Numbers { values, unit, .. }) = p.column("concentration") else {
            panic!("no concentration");
        };
        assert_eq!(values[0], 7.9);
        assert!(values[1].is_nan());
        assert_eq!(unit.as_deref(), Some("copies/µL"));
        assert!(p.column("concentration_note").is_some());
        assert!(p.notes[0].starts_with("2 cell(s)"));
    }

    #[test]
    fn amplitudes_and_calls() {
        let t = "Target Value of 0 = negative\nTarget Value of 1 = positive\nTarget Value of u = unclassified (Advanced Classification Mode)\n\nCh1Amplitude,Ch2Amplitude,1,2,\n2382.14,6258.282,0,1,\n100,200,u,0,\n";
        let p = parse(t, "dir/quant_A01_Amplitude.csv").unwrap();
        assert_eq!(p.layout, "qx-manager-amplitude");
        assert_eq!(p.well.as_deref(), Some("A01"));
        assert_eq!(p.rows, 2);
        let Some(Column::Numbers { values, .. }) = p.column("target_1") else {
            panic!("no target column");
        };
        assert_eq!(values[0], 0.0);
        assert!(values[1].is_nan());
        let p = parse("Ch1 Amplitude,Ch2 Amplitude,Cluster\n876.06,828.33,1\n", "10ng_A01_Amplitude.csv").unwrap();
        assert!(p.column("ch1_amplitude").is_some() && p.column("cluster").is_some());
    }

    #[test]
    fn wells_from_names() {
        assert_eq!(well_of("quant_A01_Amplitude.csv").as_deref(), Some("A01"));
        assert_eq!(well_of("0.0195ng_B04_Amplitude.csv").as_deref(), Some("B04"));
        assert_eq!(well_of("amplitudes.csv"), None);
    }
}
