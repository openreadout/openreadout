//! From a header row and data rows to typed columns: each header is looked up in the reader's
//! column map (our name and unit), date columns become ISO 8601 text, a `Units` column gives the
//! unit of the concentration before it, and a column is numeric when every filled cell is a
//! number.

use openreadout_core::model::Finding;

use crate::model::{Column, numbers, slug, text};
use crate::text::{concentration_unit, datetime, number};

/// What a header means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// A value column: our name and unit (`None`: no unit).
    Value(&'static str, Option<&'static str>),
    /// A concentration: its unit comes from the header (`Some`) or from the `Units` column after it.
    Concentration(&'static str, Option<&'static str>),
    /// The unit of the concentration before it.
    Units,
    /// A text column (names, ids, categories).
    Text(&'static str),
    /// A date (and time) column.
    Date(&'static str),
    /// A time column completing the date before it (NanoDrop 8000 `Date`, `Time`).
    Time,
}

/// Cells that mean "no value" as the exports write them.
fn blank(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || matches!(
            t.to_ascii_lowercase().as_str(),
            "nan" | "error" | "n/a" | "na" | "-"
        )
}

/// Normalise a header for lookup: trimmed, lower case, inner whitespace collapsed, `μ` as `µ`.
pub(crate) fn key(h: &str) -> String {
    h.replace('μ', "µ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Build the columns. `role` maps a normalised header to its role (`None`: kept under a name made
/// from the header). `month_first` reads ambiguous dates (`date_hint` may settle them per row).
pub(crate) fn build(
    headers: &[String],
    rows: &[Vec<String>],
    role: &dyn Fn(&str) -> Option<Role>,
    month_first: bool,
    date_hint: Option<&dyn Fn(usize) -> Option<String>>,
    findings: &mut Vec<Finding>,
) -> Vec<Column> {
    let roles: Vec<Option<Role>> = headers.iter().map(|h| role(&key(h))).collect();
    let cell = |r: &Vec<String>, k: usize| r.get(k).map_or("", String::as_str).trim().to_string();
    let mut out: Vec<Column> = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    let mut unique = |name: &str| -> String {
        let mut n = name.to_string();
        let mut i = 2;
        while !used.insert(n.clone()) {
            n = format!("{name}_{i}");
            i += 1;
        }
        n
    };
    let mut ambiguous_dates = 0usize;
    let mut k = 0;
    while k < headers.len() {
        let label = headers[k].trim().to_string();
        if label.is_empty() && rows.iter().all(|r| cell(r, k).is_empty()) {
            k += 1;
            continue;
        }
        match roles[k] {
            Some(Role::Units) | Some(Role::Time) => {
                // consumed by the column before it; alone, keep as text
                let consumed = k > 0
                    && matches!(
                        (roles[k - 1], roles[k]),
                        (Some(Role::Concentration(_, None)), Some(Role::Units))
                            | (Some(Role::Date(_)), Some(Role::Time))
                    );
                if !consumed {
                    let name = unique(&slug(&label));
                    out.push(text(
                        &name,
                        &label,
                        rows.iter().map(|r| cell(r, k)).collect(),
                    ));
                }
            }
            Some(Role::Text(n)) => {
                let name = unique(n);
                out.push(text(
                    &name,
                    &label,
                    rows.iter().map(|r| cell(r, k)).collect(),
                ));
            }
            Some(Role::Date(n)) => {
                let with_time = roles.get(k + 1) == Some(&Some(Role::Time));
                let name = unique(n);
                let values = rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let raw = if with_time {
                            format!("{} {}", cell(r, k), cell(r, k + 1))
                        } else {
                            cell(r, k)
                        };
                        let mf = date_hint
                            .and_then(|h| h(i))
                            .and_then(|iso| decide(&raw, &iso))
                            .unwrap_or(month_first);
                        match datetime(&raw, mf) {
                            Some((iso, amb)) => {
                                if amb {
                                    ambiguous_dates += 1;
                                }
                                iso
                            }
                            None => raw.trim().to_string(),
                        }
                    })
                    .collect();
                out.push(text(&name, &label, values));
            }
            Some(Role::Concentration(n, unit)) => {
                let raw: Vec<Option<f64>> = rows.iter().map(|r| number(&cell(r, k))).collect();
                let units: Vec<String> = match unit {
                    Some(u) => vec![u.to_string(); rows.len()],
                    None if roles.get(k + 1) == Some(&Some(Role::Units)) => {
                        rows.iter().map(|r| cell(r, k + 1)).collect()
                    }
                    None => vec![String::new(); rows.len()],
                };
                let name = unique(n);
                let (values, unit) = one_unit(&raw, &units, &name, findings);
                out.push(numbers(&name, &label, unit.as_deref(), values));
                // words in place of a number (`Out of range`, `Too low`): kept beside it
                let words: Vec<String> = rows
                    .iter()
                    .map(|r| {
                        let c = cell(r, k);
                        if number(&c).is_some() || c.trim().is_empty() {
                            String::new()
                        } else {
                            c
                        }
                    })
                    .collect();
                if words.iter().any(|w| !w.is_empty()) {
                    let note = unique(&format!("{name}_note"));
                    out.push(text(
                        &note,
                        &format!("{label} (text in place of a number)"),
                        words,
                    ));
                }
            }
            Some(Role::Value(n, unit)) => {
                let name = unique(n);
                out.push(typed(&name, &label, unit, rows, k, &cell));
            }
            None => {
                let name = unique(&slug(&label));
                out.push(typed(&name, &label, None, rows, k, &cell));
            }
        }
        k += 1;
    }
    if ambiguous_dates > 0 {
        findings.push(Finding::info(
            "date_order_assumed",
            format!(
                "{ambiguous_dates} date(s) read {} (day and month both 12 or less)",
                if month_first {
                    "month first"
                } else {
                    "day first"
                }
            ),
        ));
    }
    out
}

/// Day and month order that turns `raw` into the date of `iso` (`yyyy-mm-dd`), if one does.
fn decide(raw: &str, iso: &str) -> Option<bool> {
    let want = iso.get(..10)?;
    for mf in [true, false] {
        if datetime(raw, mf).is_some_and(|(d, _)| d.get(..10) == Some(want)) {
            return Some(mf);
        }
    }
    None
}

/// A value column: numbers when every filled cell is one, else text.
fn typed(
    name: &str,
    label: &str,
    unit: Option<&str>,
    rows: &[Vec<String>],
    k: usize,
    cell: &dyn Fn(&Vec<String>, usize) -> String,
) -> Column {
    let cells: Vec<String> = rows.iter().map(|r| cell(r, k)).collect();
    let parsed: Vec<Option<f64>> = cells.iter().map(|c| number(c)).collect();
    let numeric = parsed.iter().any(Option::is_some)
        && cells
            .iter()
            .zip(&parsed)
            .all(|(c, p)| p.is_some() || blank(c));
    if numeric {
        numbers(
            name,
            label,
            unit,
            parsed.into_iter().map(|v| v.unwrap_or(f64::NAN)).collect(),
        )
    } else {
        text(name, label, cells)
    }
}

/// Concentrations in one unit: the most common unit of the rows; rows in another
/// mass-per-volume unit are converted, rows in an unknown one become NaN (with a finding).
fn one_unit(
    raw: &[Option<f64>],
    units: &[String],
    name: &str,
    findings: &mut Vec<Finding>,
) -> (Vec<f64>, Option<String>) {
    let parsed: Vec<Option<(String, f64)>> = units.iter().map(|u| concentration_unit(u)).collect();
    // units in order of first appearance, with their counts (ties go to the first)
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (p, v) in parsed.iter().zip(raw) {
        if let (Some((u, _)), Some(_)) = (p, v) {
            match counts.iter_mut().find(|(x, _)| *x == u.as_str()) {
                Some(c) => c.1 += 1,
                None => counts.push((u.as_str(), 1)),
            }
        }
    }
    let best = counts.iter().map(|c| c.1).max().unwrap_or(0);
    let Some(target) = counts
        .iter()
        .find(|c| c.1 == best)
        .map(|(u, _)| (*u).to_string())
    else {
        // no unit the reader knows: values as stored, the unit as written when it is one
        let written: std::collections::BTreeSet<&str> = units
            .iter()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .collect();
        let unit = (written.len() == 1)
            .then(|| written.iter().next().map(|u| (*u).to_string()))
            .flatten();
        return (raw.iter().map(|v| v.unwrap_or(f64::NAN)).collect(), unit);
    };
    let tf = concentration_unit(&target).map_or(1.0, |t| t.1);
    let mut converted = 0usize;
    let mut unknown = 0usize;
    let values = raw
        .iter()
        .zip(&parsed)
        .map(|(v, p)| match (v, p) {
            (Some(v), Some((u, f))) => {
                if *u != target {
                    converted += 1;
                }
                v * f / tf
            }
            (Some(_), None) => {
                unknown += 1;
                f64::NAN
            }
            (None, _) => f64::NAN,
        })
        .collect();
    if converted > 0 {
        findings.push(Finding::info(
            "units_converted",
            format!(
                "{converted} `{name}` value(s) converted to {target}, the unit of the other rows"
            ),
        ));
    }
    if unknown > 0 {
        findings.push(Finding::warning(
            "unit_not_recognised",
            format!("{unknown} `{name}` value(s) have a unit that is not a mass per volume; they are NaN"),
        ));
    }
    (values, Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn builds_typed_columns() {
        let headers = s(&["Sample", "Conc.", "Units", "Date", "Time", "Odd"]);
        let rows = vec![
            s(&["a", "1.5", "ng/ul", "5/26/2020", "3:36 PM", "x"]),
            s(&["b", "2", "ug/ml", "5/26/2020", "3:37 PM", "1"]),
        ];
        let role = |k: &str| match k {
            "sample" => Some(Role::Text("sample")),
            "conc." => Some(Role::Concentration("concentration", None)),
            "units" => Some(Role::Units),
            "date" => Some(Role::Date("measured_at")),
            "time" => Some(Role::Time),
            _ => None,
        };
        let mut f = Vec::new();
        let cols = build(&headers, &rows, &role, true, None, &mut f);
        let names: Vec<&str> = cols.iter().map(Column::name).collect();
        assert_eq!(names, ["sample", "concentration", "measured_at", "odd"]);
        let Column::Numbers { values, unit, .. } = &cols[1] else {
            panic!()
        };
        assert_eq!(unit.as_deref(), Some("ng/µL"));
        assert_eq!(values, &[1.5, 2.0]);
        let Column::Text { values, .. } = &cols[2] else {
            panic!()
        };
        assert_eq!(values[0], "2020-05-26T15:36:00");
        assert!(matches!(cols[3], Column::Text { .. }));
    }
}
