//! Arbin MITS Pro result files (`.res`): a Microsoft Jet 4 database (read with
//! `openreadout_core::jet`) whose tables hold the test (`Global_Table`), every logged data point
//! (`Channel_Normal_Table`), auxiliary inputs (`Auxiliary_Table`, described by
//! `Aux_Global_Data_Table`), per-cycle statistics (`Channel_Statistic_Table`) and events
//! (`Event_Table`). Notes: `docs/formats/arbin-res.md`.

use std::collections::{BTreeMap, HashMap};

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::jet::{Jet, JetTable, JetValues, ole_date_iso};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{
    Facts, SeriesChannel, SeriesColumn, SeriesFile, SeriesTable, SeriesTrace,
};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

/// Format id of Arbin `.res` result files.
pub const FORMAT_ID: &str = "arbin-res";

/// The data-point table every Arbin result file has.
const NORMAL: &str = "Channel_Normal_Table";
/// The test table.
const GLOBAL: &str = "Global_Table";

/// Data-point columns in our vocabulary: Arbin's name, ours, unit, description.
const NORMAL_COLUMNS: &[(&str, &str, Option<&str>, &str)] = &[
    ("Test_Time", "time", Some("s"), "test time"),
    (
        "Step_Time",
        "step_time",
        Some("s"),
        "time since the step started",
    ),
    (
        "DateTime",
        "date_time",
        Some("d"),
        "date and time of the point: days since 1899-12-30, local time of the tester",
    ),
    (
        "Data_Point",
        "record",
        None,
        "the point's number in the test",
    ),
    (
        "Step_Index",
        "step_index",
        None,
        "the schedule step the point belongs to",
    ),
    (
        "Cycle_Index",
        "cycle",
        None,
        "cycle number (1-based, as stored)",
    ),
    (
        "Is_FC_Data",
        "fc_data",
        None,
        "1 for a point logged by a formula/fast-capture condition",
    ),
    (
        "Current",
        "current",
        Some("A"),
        "current (negative while discharging)",
    ),
    ("Voltage", "voltage", Some("V"), "cell voltage"),
    (
        "Charge_Capacity",
        "charge_capacity",
        Some("A·h"),
        "charge capacity of the cycle",
    ),
    (
        "Discharge_Capacity",
        "discharge_capacity",
        Some("A·h"),
        "discharge capacity of the cycle",
    ),
    (
        "Charge_Energy",
        "charge_energy",
        Some("W·h"),
        "charge energy of the cycle",
    ),
    (
        "Discharge_Energy",
        "discharge_energy",
        Some("W·h"),
        "discharge energy of the cycle",
    ),
    (
        "dV/dt",
        "dv_dt",
        Some("V/s"),
        "rate of change of the voltage",
    ),
    (
        "Internal_Resistance",
        "internal_resistance",
        Some("Ω"),
        "internal resistance (last measured)",
    ),
    (
        "AC_Impedance",
        "ac_impedance",
        Some("Ω"),
        "AC impedance (last measured)",
    ),
    (
        "ACI_Phase_Angle",
        "aci_phase_angle",
        None,
        "phase angle of the AC impedance",
    ),
];

/// A name made from an Arbin column label: lower case, runs of other characters as `_`.
/// A capital after a lower-case letter starts a word (`PulseStageIndex` is `pulse_stage_index`).
fn snake(label: &str) -> String {
    let mut s = String::with_capacity(label.len() + 4);
    let mut prev_lower = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && prev_lower && !s.ends_with('_') {
                s.push('_');
            }
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
            s.push(c.to_ascii_lowercase());
        } else {
            prev_lower = false;
            if !s.ends_with('_') {
                s.push('_');
            }
        }
    }
    s.trim_matches('_').to_string()
}

fn bad_db(e: Error) -> Error {
    match e {
        Error::Corrupt { detail, offset, .. } => Error::Corrupt {
            format: FORMAT_ID,
            detail,
            offset,
        },
        other => other,
    }
}

/// Text of a one-row table column (first non-blank value).
fn first_text(t: &JetTable, col: &str) -> Option<String> {
    t.texts(col)?
        .iter()
        .flatten()
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}

fn first_number(t: &JetTable, col: &str) -> Option<f64> {
    t.numbers(col)?.iter().copied().find(|v| v.is_finite())
}

/// A table's columns as series columns (numbers, texts; dates as ISO text when `dates`).
fn series_columns(t: &JetTable, order: &[usize], dates: &[&str]) -> Vec<SeriesColumn> {
    let mut out = Vec::new();
    for (c, v) in t.columns.iter().zip(&t.values) {
        let name = match c.name.as_str() {
            "Test_ID" => "test".to_string(),
            "Data_Point" => "record".to_string(),
            "Test_Time" => "time".to_string(),
            "DateTime" => "date_time".to_string(),
            other => snake(other),
        };
        let mut col = match v {
            JetValues::Numbers(x) if dates.contains(&c.name.as_str()) => {
                let texts: Vec<String> = order
                    .iter()
                    .map(|&i| x.get(i).and_then(|d| ole_date_iso(*d)).unwrap_or_default())
                    .collect();
                SeriesColumn::texts(name, &texts)
            }
            JetValues::Numbers(x) => {
                let unit = if name == "time" || name.ends_with("_time") {
                    Some("s")
                } else {
                    None
                };
                SeriesColumn::numbers(
                    name,
                    unit,
                    order
                        .iter()
                        .map(|&i| x.get(i).copied().unwrap_or(f64::NAN))
                        .collect(),
                )
            }
            JetValues::Texts(x) => {
                let texts: Vec<String> = order
                    .iter()
                    .map(|&i| x.get(i).cloned().flatten().unwrap_or_default())
                    .collect();
                SeriesColumn::texts(name, &texts)
            }
            JetValues::Undecoded => continue,
        };
        col.extra.insert("label".into(), json!(c.name));
        out.push(col);
    }
    out
}

/// Row order sorted by (test, data point), ties in file order.
fn point_order(t: &JetTable) -> Vec<usize> {
    let mut order: Vec<usize> = (0..t.rows).collect();
    if let (Some(test), Some(dp)) = (t.numbers("Test_ID"), t.numbers("Data_Point")) {
        order.sort_by(|&a, &b| {
            let ka = (
                test.get(a).copied().unwrap_or(f64::NAN),
                dp.get(a).copied().unwrap_or(f64::NAN),
            );
            let kb = (
                test.get(b).copied().unwrap_or(f64::NAN),
                dp.get(b).copied().unwrap_or(f64::NAN),
            );
            ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
        });
    }
    order
}

/// Name and unit of an auxiliary channel from its data type and the unit Arbin recorded.
fn aux_name(data_type: i64, index: i64, unit: Option<&str>) -> (String, Option<String>) {
    let unit = unit.map(str::trim).filter(|u| !u.is_empty());
    let quantity = match (data_type, unit) {
        (0, _) | (_, Some("V")) => "voltage",
        (1, _) | (_, Some("C" | "°C")) => "temperature",
        _ => "",
    };
    let unit = match (quantity, unit) {
        ("temperature", Some("C") | None) => Some("°C".to_string()),
        ("voltage", None) => Some("V".to_string()),
        (_, u) => u.map(str::to_string),
    };
    let base = if quantity.is_empty() {
        format!("aux_type{data_type}")
    } else {
        format!("aux_{quantity}")
    };
    (format!("{base}_{}", index + 1), unit)
}

/// Parse an Arbin `.res` database.
///
/// # Errors
/// Not a Jet database, not an Arbin result file, or damaged tables.
pub fn parse(b: &[u8]) -> Result<SeriesFile> {
    let db = Jet::open(b, FORMAT_ID).map_err(bad_db)?;
    if !db.has_table(NORMAL) || !db.has_table(GLOBAL) {
        return Err(Error::unsupported(
            FORMAT_ID,
            "a Jet (Access) database that is not an Arbin result file",
            "open it with a database tool; an Arbin .res file has Global_Table and Channel_Normal_Table",
        ));
    }
    let mut findings = Vec::new();
    let mut observations = Observations::default();
    let mut vendor = Map::new();
    vendor.insert("database".into(), json!(db.version_name()));
    let global = db.read(GLOBAL, None).map_err(bad_db)?;
    let normal = db.read(NORMAL, None).map_err(bad_db)?;
    if normal.rows != normal.declared_rows as usize {
        findings.push(Finding::warning(
            "row_count",
            format!(
                "{NORMAL}: {} rows read, the table definition declares {}",
                normal.rows, normal.declared_rows
            ),
        ));
    }
    let version = db
        .has_table("Version_Table")
        .then(|| db.read("Version_Table", None).ok())
        .flatten()
        .and_then(|t| {
            first_text(&t, "Version_Schema_Field")
                .or_else(|| first_text(&t, "Version_Comments_Field"))
        });
    let format_version = version
        .clone()
        .unwrap_or_else(|| "Results File (no version table)".into());

    // --- the data points, one trace per test
    let order = point_order(&normal);
    let tests_col = normal.numbers("Test_ID");
    let mut tests: Vec<f64> = order
        .iter()
        .filter_map(|&i| tests_col.and_then(|t| t.get(i).copied()))
        .collect();
    tests.dedup();
    if tests.is_empty() {
        tests.push(f64::NAN);
    }
    let aux_units = aux_units(&db)?;
    let aux = if db.has_table("Auxiliary_Table") {
        Some(db.read("Auxiliary_Table", None).map_err(bad_db)?)
    } else {
        None
    };
    let mut traces = Vec::new();
    let mut duplicates = 0usize;
    let points = Points {
        normal: &normal,
        order: &order,
        tests_col,
        aux: aux.as_ref(),
        aux_units: &aux_units,
        many_tests: tests.len() > 1,
    };
    for &test in &tests {
        traces.push(test_trace(
            points,
            test,
            &mut observations,
            &mut duplicates,
        )?);
    }
    if duplicates > 0 {
        findings.push(Finding::warning(
            "aux_duplicates",
            format!(
                "{duplicates} auxiliary values repeat a data point already filled; the last is kept"
            ),
        ));
    }

    // --- tables
    let entries = table_entries(&db);
    let tables = arbin_tables(&db, &mut observations)?;
    let undecoded = undecoded_tables(&db);
    if !undecoded.is_empty() {
        findings.push(Finding::info(
            "tables_not_returned",
            format!(
                "tables with rows that are not returned as traces or tables: {}",
                undecoded.join(", ")
            ),
        ));
    }
    if let Some(m) = resume(&db) {
        vendor.insert("resume".into(), Value::Object(m));
    }

    // --- facts
    let g = global_json(&global);
    if global.rows > 1 {
        findings.push(Finding::info(
            "tests",
            format!(
                "{} tests in the file; experiment facts are the first test's",
                global.rows
            ),
        ));
    }
    vendor.insert("global".into(), Value::Object(g));
    let facts = arbin_facts(&global, &traces, &mut observations);
    observations.feature(
        FeatureKind::FormatVersion,
        &format_version,
        &[Scope::Metadata, Scope::Traces, Scope::Tables],
    );
    observations.feature(
        FeatureKind::Layout,
        format!("{} database", db.version_name()),
        &[Scope::Traces, Scope::Tables],
    );
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    provenance.insert("tables".into(), Source::PriorArt);
    vendor.insert("file_size".into(), json!(b.len()));
    Ok(SeriesFile {
        format_version: Some(format_version),
        traces,
        tables,
        experiment: Some(facts.build()),
        vendor: Value::Object(vendor),
        entries,
        findings,
        notes: vec![
            "Values are as the tester stored them: current in A, capacities in A·h, energies in W·h; `date_time` in days since 1899-12-30 (tester local time)".into(),
        ],
        provenance,
        observations,
        checks: vec![
            "Jet catalog and table definitions".into(),
            "row bounds and null masks".into(),
            "row counts against the table definitions".into(),
        ],
        members: Vec::new(),
    })
}

/// The data-point tables a test's trace is built from.
#[derive(Clone, Copy)]
struct Points<'a> {
    normal: &'a JetTable,
    order: &'a [usize],
    tests_col: Option<&'a [f64]>,
    aux: Option<&'a JetTable>,
    aux_units: &'a BTreeMap<(i64, i64), String>,
    many_tests: bool,
}

/// The trace of one test: its data points in order, the abscissa first.
fn test_trace(
    points: Points<'_>,
    test: f64,
    observations: &mut Observations,
    duplicates: &mut usize,
) -> Result<SeriesTrace> {
    let Points {
        normal,
        order,
        tests_col,
        aux,
        aux_units,
        many_tests,
    } = points;
    let rows: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| tests_col.is_none_or(|t| t.get(i).is_some_and(|v| v.total_cmp(&test).is_eq())))
        .collect();
    let n = rows.len();
    let mut channels: Vec<(SeriesChannel, Vec<f64>)> = Vec::new();
    for (c, v) in normal.columns.iter().zip(&normal.values) {
        let JetValues::Numbers(x) = v else { continue };
        if c.name == "Test_ID" {
            continue;
        }
        let known = NORMAL_COLUMNS.iter().find(|k| k.0 == c.name);
        let (name, unit, what) = match known {
            Some(k) => (k.1.to_string(), k.2, k.3.to_string()),
            None => (snake(&c.name), None, format!("Arbin column {}", c.name)),
        };
        let mut ch = SeriesChannel::new(name, unit, "float64");
        ch.extra.insert("label".into(), json!(c.name));
        ch.extra.insert("description".into(), json!(what));
        channels.push((ch, rows.iter().map(|&i| x[i]).collect()));
        if known.is_none() {
            observations.feature(
                FeatureKind::Record,
                format!("column {}", snake(&c.name)),
                &[Scope::Traces],
            );
        }
    }
    // the abscissa first
    if let Some(k) = channels.iter().position(|(c, _)| c.name == "time") {
        let t = channels.remove(k);
        channels.insert(0, t);
    } else {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{NORMAL} has no Test_Time column"),
        ));
    }
    if let Some(aux) = aux
        && aux.rows > 0
    {
        channels.extend(aux_channels(
            aux,
            normal,
            &rows,
            test,
            aux_units,
            observations,
            duplicates,
        ));
    }
    let mut extra = BTreeMap::new();
    extra.insert(
        "axis".into(),
        json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": n}),
    );
    extra.insert("kind".into(), json!("battery_cycling"));
    if test.is_finite() {
        extra.insert("test_id".into(), json!(test));
    }
    let (names, values): (Vec<_>, Vec<_>) = channels.into_iter().unzip();
    Ok(SeriesTrace {
        name: if many_tests {
            format!("test {test}")
        } else {
            "records".into()
        },
        channels: names,
        sweeps: vec![values],
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    })
}

/// The auxiliary inputs of test `test` by data point (`rows`: the test's rows of the normal
/// table).
fn aux_channels(
    aux: &JetTable,
    normal: &JetTable,
    rows: &[usize],
    test: f64,
    aux_units: &BTreeMap<(i64, i64), String>,
    observations: &mut Observations,
    duplicates: &mut usize,
) -> Vec<(SeriesChannel, Vec<f64>)> {
    let n = rows.len();
    let mut channels = Vec::new();
    let dp_of: HashMap<i64, usize> = {
        let dp = normal.numbers("Data_Point").unwrap_or(&[]);
        #[allow(clippy::cast_possible_truncation)]
        rows.iter()
            .enumerate()
            .filter_map(|(k, &i)| dp.get(i).map(|d| (*d as i64, k)))
            .collect()
    };
    if let (Some(at), Some(adp), Some(aty), Some(aix)) = (
        aux.numbers("Test_ID"),
        aux.numbers("Data_Point"),
        aux.numbers("Data_Type"),
        aux.numbers("Auxiliary_Index"),
    ) {
        let ax = aux.numbers("X");
        let adx = aux.numbers("dX_dt");
        let mut chans: BTreeMap<(i64, i64), (Vec<f64>, Vec<f64>)> = BTreeMap::new();
        for r in 0..aux.rows {
            if !at[r].total_cmp(&test).is_eq() && test.is_finite() {
                continue;
            }
            #[allow(clippy::cast_possible_truncation)]
            let key = (aty[r] as i64, aix[r] as i64);
            #[allow(clippy::cast_possible_truncation)]
            let Some(&k) = dp_of.get(&(adp[r] as i64)) else {
                continue;
            };
            let e = chans
                .entry(key)
                .or_insert_with(|| (vec![f64::NAN; n], vec![f64::NAN; n]));
            if !e.0[k].is_nan() {
                *duplicates += 1;
            }
            e.0[k] = ax.map_or(f64::NAN, |x| x[r]);
            e.1[k] = adx.map_or(f64::NAN, |x| x[r]);
        }
        for ((ty, ix), (x, dx)) in chans {
            let (name, unit) = aux_name(ty, ix, aux_units.get(&(ty, ix)).map(String::as_str));
            let mut ch = SeriesChannel::new(name.clone(), unit.as_deref(), "float64");
            ch.extra.insert(
                "label".into(),
                json!(format!(
                    "Auxiliary_Table X (Data_Type {ty}, Auxiliary_Index {ix})"
                )),
            );
            ch.extra.insert(
                "description".into(),
                json!("auxiliary input at the data point"),
            );
            observations.feature(
                FeatureKind::Record,
                format!(
                    "column {}",
                    name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_')
                ),
                &[Scope::Traces],
            );
            channels.push((ch, x));
            if dx.iter().any(|v| !v.is_nan()) {
                let du = unit.as_deref().map(|u| format!("{u}/s"));
                let mut ch = SeriesChannel::new(format!("{name}_rate"), du.as_deref(), "float64");
                ch.extra.insert(
                    "label".into(),
                    json!(format!(
                        "Auxiliary_Table dX_dt (Data_Type {ty}, Auxiliary_Index {ix})"
                    )),
                );
                ch.extra.insert(
                    "description".into(),
                    json!("rate of change of the auxiliary input"),
                );
                channels.push((ch, dx));
            }
        }
    }
    channels
}

/// Units of the auxiliary channels by (data type, index).
fn aux_units(db: &Jet<'_>) -> Result<BTreeMap<(i64, i64), String>> {
    let mut aux_units: BTreeMap<(i64, i64), String> = BTreeMap::new();
    if db.has_table("Aux_Global_Data_Table") {
        let ag = db.read("Aux_Global_Data_Table", None).map_err(bad_db)?;
        if let (Some(ty), Some(ix)) = (ag.numbers("Data_Type"), ag.numbers("Auxiliary_Index")) {
            let units = ag.texts("Unit");
            for r in 0..ag.rows {
                #[allow(clippy::cast_possible_truncation)]
                let key = (ty[r] as i64, ix[r] as i64);
                let u = units
                    .and_then(|u| u.get(r).cloned().flatten())
                    .unwrap_or_default();
                aux_units.insert(key, u);
            }
        }
    }
    Ok(aux_units)
}

fn table_entries(db: &Jet<'_>) -> Vec<LsEntry> {
    let mut entries = Vec::new();
    for name in db.tables() {
        let rows = db.declared_rows(name).unwrap_or(0);
        entries.push(LsEntry {
            kind: "table".into(),
            name: name.to_string(),
            offset: None,
            size: None,
            image: None,
            details: json!({"rows": rows}),
        });
    }
    entries
}

/// The statistics and event tables.
fn arbin_tables(db: &Jet<'_>, observations: &mut Observations) -> Result<Vec<SeriesTable>> {
    let mut tables = Vec::new();
    for (arbin, ours, what, dates) in [
        (
            "Channel_Statistic_Table",
            "statistics",
            "per-cycle statistics Arbin logged at the data point named in `record`",
            &[][..],
        ),
        (
            "Event_Table",
            "events",
            "test events (start, end, pauses, ...) with the date and test time",
            &["DateTime"][..],
        ),
    ] {
        if !db.has_table(arbin) {
            continue;
        }
        let t = db.read(arbin, None).map_err(bad_db)?;
        if t.rows == 0 {
            continue;
        }
        let order = point_order(&t);
        let mut extra = BTreeMap::new();
        extra.insert("description".into(), json!(what));
        extra.insert("label".into(), json!(arbin));
        tables.push(SeriesTable {
            name: ours.into(),
            columns: series_columns(&t, &order, dates),
            extra,
        });
        observations.feature(
            FeatureKind::Record,
            format!("table {ours}"),
            &[Scope::Tables],
        );
    }
    Ok(tables)
}

/// Tables with rows that are not returned.
fn undecoded_tables(db: &Jet<'_>) -> Vec<String> {
    let mut undecoded = Vec::new();
    for name in db.tables() {
        if matches!(
            name,
            NORMAL
                | GLOBAL
                | "Channel_Statistic_Table"
                | "Event_Table"
                | "Auxiliary_Table"
                | "Aux_Global_Data_Table"
                | "Version_Table"
                | "Resume_Table"
        ) {
            continue;
        }
        if db.declared_rows(name).unwrap_or(0) > 0 {
            undecoded.push(name.to_string());
        }
    }
    undecoded
}

/// The first row of the resume table.
fn resume(db: &Jet<'_>) -> Option<Map<String, Value>> {
    if !db.has_table("Resume_Table") {
        return None;
    }
    let t = db.read("Resume_Table", None).ok()?;
    let mut m = Map::new();
    for (c, v) in t.columns.iter().zip(&t.values) {
        match v {
            JetValues::Numbers(x) if !x.is_empty() => {
                m.insert(c.name.clone(), openreadout_core::series::json_num(x[0]));
            }
            JetValues::Texts(x) if !x.is_empty() => {
                m.insert(c.name.clone(), json!(x[0]));
            }
            _ => {}
        }
    }
    Some(m)
}

/// The global table's first row (the start date as ISO-8601).
fn global_json(global: &JetTable) -> Map<String, Value> {
    let mut g = Map::new();
    for (c, v) in global.columns.iter().zip(&global.values) {
        match v {
            JetValues::Numbers(x) if !x.is_empty() => {
                let val = if c.name == "Start_DateTime" {
                    ole_date_iso(x[0]).map_or(Value::Null, Value::String)
                } else {
                    openreadout_core::series::json_num(x[0])
                };
                g.insert(c.name.clone(), val);
            }
            JetValues::Texts(x) if !x.is_empty() => {
                g.insert(c.name.clone(), json!(x[0]));
            }
            _ => {}
        }
    }
    g
}

/// Experiment facts of the global table and the traces.
fn arbin_facts(
    global: &JetTable,
    traces: &[SeriesTrace],
    observations: &mut Observations,
) -> Facts {
    let mut facts = Facts::new();
    facts.set("instrument.vendor", "Arbin Instruments", "the file format");
    facts.set("instrument.software", "Arbin MITS Pro", "the file format");
    if let Some(v) = first_text(global, "Software_Version") {
        facts.set(
            "instrument.software_version",
            &v,
            "Global_Table Software_Version",
        );
        observations.feature(
            FeatureKind::WriterVersion,
            format!("MITS Pro {}", v.split_whitespace().next().unwrap_or(&v)),
            &[Scope::Metadata],
        );
    }
    if let Some(v) = first_text(global, "Serial_Number") {
        facts.set("instrument.serial", &v, "Global_Table Serial_Number");
    }
    if let Some(v) = first_text(global, "Test_Name") {
        facts.set("sample.name", &v, "Global_Table Test_Name");
    }
    if let Some(v) = first_text(global, "Item_ID") {
        facts.set("sample.id", &v, "Global_Table Item_ID");
    }
    if let Some(v) = first_text(global, "Creator") {
        facts.set("acquisition.operator", &v, "Global_Table Creator");
    }
    if let Some(v) = first_text(global, "Comments") {
        facts.set("acquisition.comment", &v, "Global_Table Comments");
    }
    if let Some(v) = first_text(global, "Schedule_File_Name") {
        facts.set("method.name", &v, "Global_Table Schedule_File_Name");
    }
    if let Some(s) = first_number(global, "Start_DateTime").and_then(ole_date_iso) {
        facts.set(
            "acquisition.started_at",
            &s,
            "Global_Table Start_DateTime (tester local time)",
        );
    }
    for (col, name, unit) in [
        ("MASS", "mass", "g"),
        ("Specific_Capacity", "specific_capacity", "A·h/g"),
        ("Capacity", "nominal_capacity", "A·h"),
    ] {
        if let Some(v) = first_number(global, col)
            && v > 0.0
        {
            facts.number(name, v, unit, &format!("Global_Table {col}"));
        }
    }
    if let Some(v) = first_number(global, "Channel_Index") {
        facts.plain(
            "channel",
            openreadout_core::series::json_num(v),
            "Global_Table Channel_Index",
        );
    }
    let first = traces.first();
    let n_total: usize = traces
        .iter()
        .map(|t| t.sweeps[0].first().map_or(0, Vec::len))
        .sum();
    if let Some(t) = first
        && let Some(times) = t.sweeps[0].first()
        && let Some(last) = times.iter().rev().find(|v| v.is_finite())
        && *last > 0.0
    {
        facts.duration(*last, "the last data point's test time");
    }
    let cycles = traces
        .iter()
        .filter_map(|t| {
            let k = t.channels.iter().position(|c| c.name == "cycle")?;
            t.sweeps[0][k]
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .reduce(f64::max)
        })
        .fold(0.0f64, f64::max);
    for k in 0..traces.len() {
        #[allow(clippy::cast_possible_truncation)]
        facts.measurement(
            MeasurementKind::Trace,
            vec![k as u32],
            if cycles > 0.0 {
                format!("battery cycling, {n_total} data points, {cycles} cycles")
            } else {
                format!("battery cycling, {n_total} data points")
            },
            None,
        );
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(snake("Charge_Time"), "charge_time");
        assert_eq!(snake("PulseStageIndex"), "pulse_stage_index");
        assert_eq!(snake("ACR"), "acr");
        assert_eq!(snake("MASS"), "mass");
        assert_eq!(snake("TC_Counter1"), "tc_counter1");
        assert_eq!(snake("Vmax_On_Cycle"), "vmax_on_cycle");
        assert_eq!(
            aux_name(0, 0, Some("V")),
            ("aux_voltage_1".into(), Some("V".into()))
        );
        assert_eq!(
            aux_name(1, 2, Some("C")),
            ("aux_temperature_3".into(), Some("°C".into()))
        );
        assert_eq!(aux_name(4, 0, Some("")), ("aux_type4_1".into(), None));
    }
}
