//! Thermo Fisher NanoDrop exports. Layouts (`docs/formats/nanodrop.md`):
//!
//! - `nanodrop-8000`: the NanoDrop 8000 (and 2000) software's tab-separated report, header row
//!   `Plate ID`, `Well`, `Sample ID`, `User ID`, `Date`, `Time`, then the application's columns
//!   (nucleic acid, protein A280, custom methods). A second table (standards) may follow a
//!   blank line.
//! - `nanodrop-eight`: NanoDrop Eight data export: `Key:<tab>value` lines (`Application`,
//!   `Serial Number`, `User Name`), then a tab-separated table whose columns after the results
//!   are the absorbance spectrum, one column per wavelength.
//! - `nanodrop-one`: NanoDrop One/OneC comma-separated report (`Date`, `Sample Name`, results).
//! - `nanodrop-spectra`: NanoDrop One UV-Vis spectrum export: blocks of a sample line, a date
//!   line, `Wavelength (nm)<tab>10mm Absorbance`, `//`-prefixed calibration lines, then
//!   wavelength–absorbance pairs.

use openreadout_core::model::Finding;
use openreadout_core::{Error, Result};

use crate::model::{Parsed, Spectra, numbers, text};
use crate::table::{Role, build, key};
use crate::text::{datetime, number, split};

pub(crate) const FORMAT_ID: &str = "nanodrop";
/// Largest export read (a year of 96-sample spectra is a few MB).
pub(crate) const MAX_BYTES: u64 = 64 << 20;
/// Most spectrum points accepted per sample.
const MAX_POINTS: usize = 100_000;
/// Most samples accepted.
const MAX_ROWS: usize = 1_000_000;

/// A line with each tab-separated field normalised by [`key`].
fn line_key(l: &str) -> String {
    l.split('\t').map(key).collect::<Vec<_>>().join("\t")
}

/// Which layout the head of a file is, if a NanoDrop one.
pub(crate) fn sniff(text: &str) -> Option<&'static str> {
    let mut lines = text.lines().map(|l| l.trim_end_matches(['\r', '\t']));
    let first = lines.next()?.trim_start_matches('\u{feff}');
    let k = line_key(first);
    if k.starts_with("plate id\twell")
        || (k.starts_with("sample id\tuser id\tdate") && k.contains("260/280"))
    {
        return Some("nanodrop-8000");
    }
    if k.starts_with("application:\t") {
        let table = text.lines().take(12).any(|l| {
            let l = line_key(l);
            (l.contains("a260/a280") || l.contains("a280")) && l.matches('\t').count() >= 4
        });
        if table {
            return Some("nanodrop-eight");
        }
    }
    if k.starts_with("date,sample name,")
        && (k.contains("a260/a280") || k.contains("a280") || k.contains("nucleic acid"))
    {
        return Some("nanodrop-one");
    }
    // a spectrum block: name, date, the column titles
    let head: Vec<String> = text.lines().take(3).map(line_key).collect();
    if head.len() == 3 && head[2].starts_with("wavelength (nm)\t") && head[2].contains("absorbance")
    {
        return Some("nanodrop-spectra");
    }
    None
}

fn role(k: &str) -> Option<Role> {
    use Role::{Concentration as C, Date, Text as T, Time, Units, Value as V};
    Some(match k {
        "plate id" => T("plate"),
        "well" => T("well"),
        "location" => T("location"),
        "user id" | "username" | "user name" | "user" => T("user"),
        "sample name" | "sample" | "sample id" => T("sample"),
        "uid" => T("sample_uid"),
        "date & time" | "date" => Date("measured_at"),
        "time" => Time,
        "conc." => C("concentration", None),
        "units" => Units,
        "ng/µl"
        | "ng/ul"
        | "nucleic acid(ng/ul)"
        | "nucleic acid (ng/ul)"
        | "nucleic acid(ng/µl)" => C("concentration", Some("ng/µL")),
        "protein (mg/ml)" | "mg/ml" | "protein(mg/ml)" => C("concentration", Some("mg/mL")),
        "a260" => V("a260", None),
        "a280" | "a280 10mm" => V("a280", None),
        "a230" => V("a230", None),
        "260/280" | "a260/a280" => V("a260_a280", None),
        "260/230" | "a260/a230" => V("a260_a230", None),
        "conc. factor (ng/ul)" | "nucleic acid factor" | "factor" => V("factor", None),
        "e 1%" => V("e1_percent", None),
        "ext. coeff x10e3" => V("extinction_coefficient_1e3", None),
        "ext.c. (l/(mol*cm))" => V("extinction_coefficient", Some("L/(mol·cm)")),
        "mol. wt. kda" => V("molecular_weight", Some("kDa")),
        "m.w. (g/mol)" => V("molecular_weight", Some("g/mol")),
        "cursor pos." | "cursor nm" => V("cursor_wavelength", Some("nm")),
        "cursor abs." => V("cursor_absorbance", None),
        "340 raw" => V("a340_raw", None),
        "na type" => T("nucleic_acid_type"),
        "baseline correction (nm)" => V("baseline_wavelength", Some("nm")),
        "baseline absorbance" => V("baseline_absorbance", None),
        "corrected (ng/ul)" => C("corrected_concentration", Some("ng/µL")),
        "corrected %cv" => V("corrected_cv", Some("%")),
        "nm 1" => V("wavelength_1", Some("nm")),
        "nm 2" => V("wavelength_2", Some("nm")),
        "nm 3" => V("wavelength_3", Some("nm")),
        "nm 4" => V("wavelength_4", Some("nm")),
        "abs 1" => V("absorbance_1", None),
        "abs 2" => V("absorbance_2", None),
        "abs 3" => V("absorbance_3", None),
        "abs 4" => V("absorbance_4", None),
        "formula name 1" => T("formula_name_1"),
        "formula 1" => T("formula_1"),
        "formula value 1" => V("formula_value_1", None),
        _ => return None,
    })
}

/// Rows of a delimited table, ragged rows padded or flagged.
fn rows_of(lines: &[&str], delim: char, n: usize, findings: &mut Vec<Finding>) -> Vec<Vec<String>> {
    let mut long = 0usize;
    let mut out = Vec::new();
    for l in lines {
        if l.trim().is_empty() {
            continue;
        }
        let mut f = split(l, delim);
        while f.len() > n && f.last().is_some_and(|x| x.trim().is_empty()) {
            f.pop();
        }
        if f.len() > n {
            long += 1;
        }
        out.push(f);
    }
    if long > 0 {
        findings.push(Finding::warning(
            "ragged_rows",
            format!("{long} row(s) have more cells than the header has columns; the extra cells are not read"),
        ));
    }
    out
}

/// Parse a whole export.
pub(crate) fn parse(text: &str) -> Result<Parsed> {
    let layout = sniff(text).ok_or_else(|| {
        Error::corrupt(FORMAT_ID, "not a NanoDrop export layout this reader knows")
    })?;
    match layout {
        "nanodrop-spectra" => parse_spectra(text),
        "nanodrop-eight" => parse_eight(text),
        "nanodrop-one" => parse_table(text, layout, ','),
        _ => parse_table(text, layout, '\t'),
    }
}

/// The 8000 report and the One CSV: a header row and rows, up to the first blank line.
fn parse_table(text: &str, layout: &'static str, delim: char) -> Result<Parsed> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let headers: Vec<String> = split(lines[0].trim_start_matches('\u{feff}'), delim);
    let end = lines
        .iter()
        .skip(1)
        .position(|l| l.trim().is_empty())
        .map_or(lines.len(), |i| i + 1);
    let mut p = Parsed {
        layout,
        model: Some(
            match layout {
                "nanodrop-one" => "NanoDrop One",
                _ => "NanoDrop 8000",
            }
            .into(),
        ),
        ..Parsed::default()
    };
    let rows = rows_of(&lines[1..end], delim, headers.len(), &mut p.findings);
    if rows.len() > MAX_ROWS {
        return Err(Error::corrupt(FORMAT_ID, format!("{} rows", rows.len())));
    }
    if end < lines.len() && lines[end..].iter().any(|l| !l.trim().is_empty()) {
        // the 8000's standards table, kept in the vendor tree as text
        let rest: Vec<&str> = lines[end..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();
        p.header.push(("second table".into(), rest.join("\n")));
        p.notes.push(format!(
            "a second table ({} lines, the standards of a standard curve) follows the samples; it is in the vendor metadata, not in table 0",
            rest.len()
        ));
    }
    p.columns = build(&headers, &rows, &role, true, None, &mut p.findings);
    p.rows = rows.len();
    p.operator = p
        .first_text("user")
        .filter(|u| *u != "Default")
        .map(str::to_string);
    p.application = application_of(&p);
    Ok(p)
}

/// The application, from the columns the export has.
fn application_of(p: &Parsed) -> Option<String> {
    if let Some(t) = p.first_text("nucleic_acid_type") {
        return Some(t.to_string());
    }
    if p.column("formula_name_1").is_some() || p.column("wavelength_1").is_some() {
        return Some("Custom method".into());
    }
    if p.column("e1_percent").is_some() || p.column("extinction_coefficient_1e3").is_some() {
        return Some("Protein A280".into());
    }
    if p.column("a260").is_some() || p.column("a260_a230").is_some() {
        return Some("Nucleic acid".into());
    }
    None
}

/// The Eight export: key lines, a header row, rows whose wavelength columns are the spectrum.
fn parse_eight(text: &str) -> Result<Parsed> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut p = Parsed {
        layout: "nanodrop-eight",
        model: Some("NanoDrop Eight".into()),
        ..Parsed::default()
    };
    let mut k = 0;
    while k < lines.len() {
        let f: Vec<String> = split(lines[k], '\t')
            .into_iter()
            .filter(|c| !c.trim().is_empty())
            .collect();
        if f.first().is_some_and(|c| c.trim_end().ends_with(':')) && f.len() <= 2 {
            let name = f[0].trim().trim_end_matches(':').to_string();
            let value = f.get(1).map_or(String::new(), |v| v.trim().to_string());
            match key(&name).as_str() {
                "application" => p.application = Some(value.clone()),
                "serial number" => p.serial = Some(value.clone()).filter(|v| !v.is_empty()),
                "user name" => p.operator = Some(value.clone()).filter(|v| !v.is_empty()),
                _ => {}
            }
            p.header.push((name, value));
            k += 1;
        } else {
            break;
        }
    }
    if k >= lines.len() {
        return Err(Error::corrupt(FORMAT_ID, "no table after the header lines"));
    }
    let mut headers = split(lines[k], '\t');
    while headers.last().is_some_and(|h| h.trim().is_empty()) {
        headers.pop();
    }
    // wavelength columns: headers that are numbers between 150 and 1100 nm
    let wl: Vec<(usize, f64)> = headers
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            number(h)
                .filter(|v| (150.0..=1100.0).contains(v))
                .map(|v| (i, v))
        })
        .collect();
    let rows = rows_of(&lines[k + 1..], '\t', headers.len(), &mut p.findings);
    if rows.len() > MAX_ROWS || wl.len() > MAX_POINTS {
        return Err(Error::corrupt(
            FORMAT_ID,
            "more rows or wavelengths than any export holds",
        ));
    }
    let first_wl = wl.first().map_or(usize::MAX, |w| w.0);
    let result_headers: Vec<String> = headers[..first_wl.min(headers.len())].to_vec();
    // the sample column of the Eight is `Sample Name`; its `Sample Id` is a unique id
    let eight_role = |k: &str| {
        if k == "sample id" {
            Some(Role::Text("sample_uid"))
        } else {
            role(k)
        }
    };
    p.columns = build(
        &result_headers,
        &rows,
        &eight_role,
        true,
        None,
        &mut p.findings,
    );
    p.rows = rows.len();
    if wl.len() >= 2 {
        let step = (wl[wl.len() - 1].1 - wl[0].1) / (wl.len() - 1) as f64;
        let regular = wl
            .windows(2)
            .all(|w| ((w[1].1 - w[0].1) - step).abs() < 1e-6 && w[1].0 == w[0].0 + 1);
        if regular && step > 0.0 {
            p.spectra = Some(Spectra {
                first_nm: wl[0].1,
                step_nm: step,
                y_label: "absorbance".into(),
                rows: rows
                    .iter()
                    .map(|r| {
                        wl.iter()
                            .map(|(i, _)| r.get(*i).and_then(|c| number(c)).unwrap_or(f64::NAN))
                            .collect()
                    })
                    .collect(),
            });
        } else {
            p.findings.push(Finding::warning(
                "spectrum_not_read",
                "the wavelength columns are not evenly spaced; the spectra are not returned",
            ));
        }
    }
    Ok(p)
}

/// The One's UV-Vis spectrum export: one block per measurement.
#[allow(clippy::many_single_char_names)] // k, n, p, x, y: line, count, point, axes
fn parse_spectra(src: &str) -> Result<Parsed> {
    let lines: Vec<&str> = src.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut names = Vec::new();
    let mut times = Vec::new();
    let mut spectra: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut calib: Vec<String> = Vec::new();
    let mut y_label = String::from("10mm Absorbance");
    let mut k = 0;
    let mut p = Parsed {
        layout: "nanodrop-spectra",
        model: Some("NanoDrop One".into()),
        application: Some("UV-Vis".into()),
        ..Parsed::default()
    };
    while k < lines.len() {
        if lines[k].trim().is_empty() {
            k += 1;
            continue;
        }
        // a block: name, date, titles
        let (Some(name), Some(date), Some(titles)) =
            (lines.get(k), lines.get(k + 1), lines.get(k + 2))
        else {
            p.findings.push(Finding::error(
                "truncated_block",
                format!("the block at line {} ends early; it is not read", k + 1),
            ));
            break;
        };
        if !key(titles).starts_with("wavelength (nm)") {
            p.findings.push(Finding::error(
                "unexpected_block",
                format!(
                    "line {} is not the start of a spectrum block; the rest is not read",
                    k + 1
                ),
            ));
            break;
        }
        if let Some((_, y)) = titles.split_once('\t') {
            y_label = y.trim().to_string();
        }
        names.push(name.trim().to_string());
        times.push(datetime(date, true).map_or_else(|| date.trim().to_string(), |d| d.0));
        let mut pts = Vec::new();
        k += 3;
        while k < lines.len() && !lines[k].trim().is_empty() {
            let l = lines[k];
            if let Some(c) = l.strip_prefix("//") {
                if spectra.is_empty() {
                    calib.push(c.trim().replace('\t', " ").trim().to_string());
                }
            } else {
                let f: Vec<&str> = l.split('\t').collect();
                match (
                    f.first().and_then(|x| number(x)),
                    f.get(1).and_then(|x| number(x)),
                ) {
                    (Some(x), Some(y)) => pts.push((x, y)),
                    _ => {
                        p.findings.push(Finding::warning(
                            "unreadable_line",
                            format!(
                                "line {} is not a wavelength and an absorbance; skipped",
                                k + 1
                            ),
                        ));
                    }
                }
                if pts.len() > MAX_POINTS {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        "a spectrum longer than any instrument writes",
                    ));
                }
            }
            k += 1;
        }
        spectra.push(pts);
        if spectra.len() > MAX_ROWS {
            return Err(Error::corrupt(
                FORMAT_ID,
                "more spectra than any export holds",
            ));
        }
    }
    if spectra.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "no spectrum blocks"));
    }
    // one axis for all: the first block's, when every block shares it
    let axis: Vec<f64> = spectra[0].iter().map(|p| p.0).collect();
    let n = axis.len();
    let step = if n > 1 {
        (axis[n - 1] - axis[0]) / (n - 1) as f64
    } else {
        0.0
    };
    let regular =
        n > 1 && step > 0.0 && axis.windows(2).all(|w| ((w[1] - w[0]) - step).abs() < 1e-6);
    let same = spectra
        .iter()
        .all(|s| s.len() == n && s.iter().zip(&axis).all(|(a, b)| (a.0 - b).abs() < 1e-9));
    if !(regular && same) {
        return Err(Error::unsupported(
            FORMAT_ID,
            "a spectrum export whose blocks do not share one evenly spaced wavelength axis",
            "Export the spectra again from the NanoDrop software with one wavelength range, or split the file per sample.",
        ));
    }
    let at = |s: &[(f64, f64)], wl: f64| -> f64 {
        s.iter()
            .find(|p| (p.0 - wl).abs() < 1e-6)
            .map_or(f64::NAN, |p| p.1)
    };
    let col = |wl: f64| spectra.iter().map(|s| at(s, wl)).collect::<Vec<f64>>();
    let (a230, a260, a280) = (col(230.0), col(260.0), col(280.0));
    let ratio = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x / y).collect::<Vec<f64>>();
    p.columns = vec![
        text("sample", "sample line of the block", names),
        text("measured_at", "date line of the block", times),
        numbers(
            "a230",
            "absorbance at 230 nm, read off the spectrum",
            None,
            a230.clone(),
        ),
        numbers(
            "a260",
            "absorbance at 260 nm, read off the spectrum",
            None,
            a260.clone(),
        ),
        numbers(
            "a280",
            "absorbance at 280 nm, read off the spectrum",
            None,
            a280.clone(),
        ),
        numbers(
            "a260_a280",
            "A260/A280 of the spectrum (no baseline correction)",
            None,
            ratio(&a260, &a280),
        ),
        numbers(
            "a260_a230",
            "A260/A230 of the spectrum (no baseline correction)",
            None,
            ratio(&a260, &a230),
        ),
    ];
    p.rows = spectra.len();
    p.notes.push(
        "a230, a260, a280 and their ratios are read off each spectrum at those wavelengths, without the 340 nm baseline correction of the NanoDrop nucleic-acid application".into(),
    );
    if !calib.is_empty() {
        p.header
            .push(("wavelength calibration".into(), calib.join("; ")));
    }
    p.spectra = Some(Spectra {
        first_nm: axis[0],
        step_nm: step,
        y_label,
        rows: spectra
            .iter()
            .map(|s| s.iter().map(|p| p.1).collect())
            .collect(),
    });
    Ok(p)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;
    use crate::model::Column;

    const EIGHT: &str = "Application:\tdsDNA\r\nSerial Number:\tNDE1\r\nUser Name:\tJo\r\nSample Id\tSample Name\tDate & Time\tLocation\tng/µL\tA260/A280\tA260/A230\tA260\tA280\t220.0\t220.5\t221.0\r\nu1\tS1\t06/16/2022 04:38:28 PM(-07:00)\tA\t50.5\t1.9\t2.1\t1.01\t0.53\t0.1\t0.2\t0.3\r\n";
    const P8000: &str = "Plate ID\tWell \tSample ID\tUser ID\tDate \tTime \tConc. \tUnits \tA260 \tA280 \t260/280 \t260/230 \tConc. Factor (ng/ul)\tCursor Pos.\tCursor abs.\t340 raw\tNA Type\r\n\tA1\talpha\tDefault\t4/14/2022\t3:48 PM\t9.267\tng/ul\t0.185\t0.082\t2.26\t1.39\t50.00\t260\t0.185\t0.014\tDNA-50\r\n";
    const SPEC: &str = "6 1\n5/14/2024 5:04 PM\nWavelength (nm)\t10mm Absorbance\n//WLCalib: Shift \t0.22\t\n//QSpecEnd: \n220.0\t0.1\n240.0\t1.0\n260.0\t2.0\n280.0\t1.0\n\n\n6 2\n5/14/2024 5:05 PM\nWavelength (nm)\t10mm Absorbance\n220.0\t0.1\n240.0\t0.5\n260.0\t1.0\n280.0\t0.5\n";

    #[test]
    fn eight() {
        let p = parse(EIGHT).unwrap();
        assert_eq!(p.layout, "nanodrop-eight");
        assert_eq!(p.serial.as_deref(), Some("NDE1"));
        assert_eq!(p.rows, 1);
        let s = p.spectra.as_ref().unwrap();
        assert_eq!((s.first_nm, s.step_nm), (220.0, 0.5));
        assert_eq!(s.rows[0], vec![0.1, 0.2, 0.3]);
        let Some(Column::Numbers { values, unit, .. }) = p.column("concentration") else {
            panic!()
        };
        assert_eq!((values[0], unit.as_deref()), (50.5, Some("ng/µL")));
        assert!(p.column("sample_uid").is_some());
        assert_eq!(p.first_text("sample"), Some("S1"));
    }

    #[test]
    fn report_8000() {
        let p = parse(P8000).unwrap();
        assert_eq!(p.layout, "nanodrop-8000");
        assert_eq!(p.application.as_deref(), Some("DNA-50"));
        assert_eq!(p.first_text("measured_at"), Some("2022-04-14T15:48:00"));
        assert!(p.column("a260_a280").is_some());
        assert_eq!(p.operator, None);
    }

    #[test]
    fn spectra() {
        let p = parse(SPEC).unwrap();
        assert_eq!(p.rows, 2);
        let s = p.spectra.as_ref().unwrap();
        assert_eq!(s.rows.len(), 2);
        let Some(Column::Numbers { values, .. }) = p.column("a260_a280") else {
            panic!()
        };
        assert_eq!(values, &[2.0, 2.0]);
    }

    #[test]
    fn truncations_do_not_panic() {
        for t in [EIGHT, P8000, SPEC] {
            for cut in 0..t.len() {
                if let Some(s) = t.get(..cut) {
                    let _ = parse(s);
                }
            }
        }
    }
}
