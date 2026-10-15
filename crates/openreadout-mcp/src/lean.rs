//! Lean rows for per-well results. A whole plate's growth fits, kinetic rates or qPCR records
//! repeat settings and derived numbers in every row; the full rows of a 96-well plate do not
//! fit in one reply. By default a reply keeps each row's main fields, leaves out values that
//! say nothing (false, null, empty, a role nobody assigned, a group named after its well) and
//! rounds numbers to 4 significant digits. `fields` adds other fields, and `["all"]` gives every
//! field at full precision. A reply that is still over the size limit keeps whole plate
//! rows and says which `wells` to ask for next.

use serde_json::{Map, Value};

/// When a field is left out of a row although it is a main field.
#[derive(Debug, Clone, Copy)]
pub enum Omit {
    /// The field has this value (as text, e.g. `"unassigned"` or `"0"`).
    Is(&'static str),
    /// The field has the same value as this other field of the row.
    SameAs(&'static str),
}

/// The default view of one list of rows in a result.
#[derive(Debug, Clone, Copy)]
pub struct Rows {
    /// The list's key at the top level of the result (`growth`, `records`).
    pub key: &'static str,
    /// The main fields, in the order they are worth reading.
    pub fields: &'static [&'static str],
    /// Main fields left out of a row when they carry nothing.
    pub omit: &'static [(&'static str, Omit)],
}

/// `openreadout_assay`: per-well and per-group rows.
pub const ASSAY: &[Rows] = &[
    Rows {
        key: "growth",
        fields: &[
            "well",
            "role",
            "sample",
            "group",
            "pattern",
            "growth_rate_per_h",
            "doubling_time_h",
            "lag_time_s",
            "max_value",
            "time_to_max_s",
            "exp_phase_after_crash",
            "crash_s",
            "trough_s",
            "regrowth_s",
        ],
        omit: &[
            ("role", Omit::Is("unassigned")),
            ("role", Omit::Is("sample")),
            ("group", Omit::SameAs("well")),
            ("group", Omit::SameAs("sample")),
        ],
    },
    Rows {
        key: "kinetics",
        fields: &[
            "well",
            "role",
            "sample",
            "max_slope_per_min",
            "max_slope_r_squared",
            "time_at_max_slope_s",
            "lag_time_s",
            "max_value",
            "time_to_max_s",
            "final_value",
        ],
        omit: &[
            ("role", Omit::Is("unassigned")),
            ("role", Omit::Is("sample")),
        ],
    },
    Rows {
        key: "wells",
        fields: &[
            "well",
            "role",
            "group",
            "sample",
            "compound",
            "concentration",
            "dilution",
            "raw",
            "value",
            "outlier",
            "excluded",
            "back_calculated",
            "final_concentration",
            "flag",
            "quantifiable",
            "recovery_percent",
            "percent_effect",
            "extra",
        ],
        omit: &[
            ("role", Omit::Is("unassigned")),
            ("group", Omit::SameAs("well")),
            ("group", Omit::SameAs("sample")),
            ("raw", Omit::SameAs("value")),
        ],
    },
    Rows {
        key: "samples",
        fields: &[
            "group",
            "role",
            "sample",
            "compound",
            "concentration",
            "dilution",
            "wells",
            "metric",
            "n",
            "n_used",
            "mean",
            "sd",
            "cv_percent",
            "outliers",
            "back_calculated_mean",
            "back_calculated_sd",
            "back_calculated_cv_percent",
            "back_calculated_of_mean",
            "final_concentration",
            "flag",
            "recovery_percent",
        ],
        omit: &[
            ("sample", Omit::SameAs("group")),
            ("n_used", Omit::SameAs("n")),
        ],
    },
];

/// `openreadout_qpcr`: records and ΔΔCq rows.
pub const QPCR: &[Rows] = &[
    Rows {
        key: "records",
        fields: &[
            "well",
            "sample",
            "target",
            "task",
            "cq",
            "cq_status",
            "tm",
            "excluded",
            "flags",
            "calculated_quantity",
            "vendor_rq",
            "computed_cq",
            "computed_cq_status",
        ],
        omit: &[
            ("task", Omit::Is("unknown")),
            ("cq_status", Omit::Is("determined")),
            ("computed_cq_status", Omit::Is("determined")),
        ],
    },
    Rows {
        key: "relative_quantities",
        fields: &[
            "sample",
            "target",
            "replicates",
            "undetermined",
            "cq_mean",
            "cq_sd",
            "reference_cq_mean",
            "delta_cq",
            "delta_delta_cq",
            "rq",
            "rq_min",
            "rq_max",
            "efficiency_corrected_rq",
            "vendor_rq",
        ],
        omit: &[("undetermined", Omit::Is("0"))],
    },
];

/// The `fields` argument: none (the default view), `["all"]`, or field names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fields {
    /// Main fields, rounded.
    Default,
    /// Every field at full precision.
    All,
    /// The main fields and these.
    Named(Vec<String>),
}

impl Fields {
    /// Read `fields` out of a tool's options (removing it): a list of names or one name, each
    /// of which may hold several names separated by commas (`"run, dye"`).
    pub fn take(options: &mut Map<String, Value>) -> Result<Fields, String> {
        let split = |s: &str| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .collect::<Vec<_>>()
        };
        let names: Vec<String> = match options.remove("fields") {
            None | Some(Value::Null) => return Ok(Fields::Default),
            Some(Value::String(s)) => split(&s),
            Some(Value::Array(a)) => a
                .into_iter()
                .map(|v| match v {
                    Value::String(s) => Ok(split(&s)),
                    other => Err(format!("fields: {other} is not a field name")),
                })
                .collect::<Result<Vec<_>, _>>()?
                .concat(),
            Some(other) => {
                return Err(format!(
                    "fields must be a list of field names or [\"all\"], not {other}"
                ));
            }
        };
        let names: Vec<String> = names.into_iter().filter(|n| !n.is_empty()).collect();
        if names.is_empty() {
            Ok(Fields::Default)
        } else if names.iter().any(|n| n == "all") {
            Ok(Fields::All)
        } else {
            Ok(Fields::Named(names))
        }
    }
}

/// Fields that name a row; kept with any `fields`.
const NAMES: &[&str] = &["well", "sample", "target", "group"];

/// Shape the lists of `v` named in `rows` as `fields` asks, and round the numbers of the
/// top-level values `round_also` with them. Returns a note for the reply (the fields left out
/// and how to get them, or names that matched nothing), or `None`.
pub fn shape(v: &mut Value, rows: &[Rows], round_also: &[&str], fields: &Fields) -> Option<String> {
    if *fields == Fields::All {
        return None;
    }
    let Value::Object(top) = v else {
        return None;
    };
    let extra: &[String] = match fields {
        Fields::Named(n) => n,
        _ => &[],
    };
    let mut hidden: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = extra.to_vec();
    let mut shaped: Vec<&str> = Vec::new();
    for r in rows {
        let Some(Value::Array(items)) = top.get_mut(r.key) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        shaped.push(r.key);
        for item in items.iter_mut() {
            let Value::Object(row) = item else { continue };
            for k in row.keys() {
                unknown.retain(|u| u != k);
                if !r.fields.contains(&k.as_str()) && !extra.contains(k) && !hidden.contains(k) {
                    hidden.push(k.clone());
                }
            }
            // the main fields in reading order, without those that say nothing, then the
            // fields asked for
            let mut lean = Map::new();
            for f in r.fields {
                if let Some(x) = row.get(*f)
                    && !says_nothing(f, x, row, r.omit)
                {
                    lean.insert((*f).to_string(), x.clone());
                }
            }
            for f in extra {
                if let Some(x) = row.get(f.as_str()) {
                    lean.insert(f.clone(), x.clone());
                }
            }
            for x in lean.values_mut() {
                round(x);
            }
            *row = lean;
        }
    }
    for k in round_also {
        if let Some(x) = top.get_mut(*k) {
            round(x);
        }
    }
    if shaped.is_empty() {
        return None;
    }
    let lists = match shaped.as_slice() {
        [one] => (*one).to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
        [] => String::new(),
    };
    let mut note = format!("{lists} rows show their main fields to 4 significant digits");
    if !hidden.is_empty() {
        note.push_str(&format!(
            "; fields adds others ({}), [\"all\"] gives every field at full precision",
            hidden.join(", ")
        ));
    }
    if !unknown.is_empty() {
        note.push_str(&format!("; no row has {}", unknown.join(", ")));
    }
    Some(note)
}

/// True when the value of field `k` carries nothing in this row.
fn says_nothing(k: &str, x: &Value, row: &Map<String, Value>, omit: &[(&str, Omit)]) -> bool {
    if NAMES.contains(&k) && k != "group" {
        return false;
    }
    match x {
        Value::Null | Value::Bool(false) => return true,
        Value::String(s) if s.is_empty() => return true,
        Value::Array(a) if a.is_empty() => return true,
        Value::Object(o) if o.is_empty() => return true,
        _ => {}
    }
    omit.iter().filter(|(f, _)| *f == k).any(|(_, o)| match o {
        Omit::Is(text) => match x {
            Value::String(s) => s == text,
            Value::Number(n) => n.to_string() == *text,
            _ => false,
        },
        Omit::SameAs(other) => row.get(*other).is_some_and(|y| y == x),
    })
}

/// Round every number in `x` to 4 significant digits, keeping whole numbers whole.
fn round(x: &mut Value) {
    match x {
        Value::Number(n) if n.is_f64() => {
            if let Some(f) = n.as_f64()
                && let Some(r) = serde_json::Number::from_f64(round4(f))
            {
                *n = r;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(round),
        Value::Object(o) => o.values_mut().for_each(round),
        _ => {}
    }
}

/// `f` to 4 significant digits, but never fewer than its whole-number digits.
fn round4(f: f64) -> f64 {
    if !f.is_finite() || f == 0.0 {
        return f;
    }
    let digits = f.abs().log10().floor() as i32 + 1;
    let decimals = (4 - digits).clamp(0, 15);
    let k = 10f64.powi(decimals);
    let r = (f * k).round() / k;
    if r.is_finite() { r } else { f }
}

/// When `reply` serializes to more than `max` characters, cut the per-well list `key` at a
/// plate-row boundary so that it fits, and return a note naming the wells shown and the `wells`
/// argument for the rest. Rows must be in plate order with a `well` field.
pub fn page_by_plate_row(reply: &mut Value, key: &str, max: usize) -> Option<String> {
    let size = reply.to_string().len();
    if size <= max {
        return None;
    }
    let Value::Array(items) = reply.get_mut(key)? else {
        return None;
    };
    let total = items.len();
    let sizes: Vec<usize> = items.iter().map(|x| x.to_string().len() + 1).collect();
    let list: usize = sizes.iter().sum();
    // room for the rows: what the rest of the reply leaves, less space for the note
    let room = max.saturating_sub(size - list + 300);
    let row_of = |x: &Value| {
        x.get("well")
            .and_then(Value::as_str)
            .map(|w| split_well(w).0.to_string())
    };
    let mut keep = 0usize;
    let mut used = 0usize;
    while keep < total {
        // one plate row: the run of items with the same row letters
        let letters = row_of(&items[keep]);
        let end = (keep..total)
            .find(|&k| row_of(&items[k]) != letters)
            .unwrap_or(total);
        let row_size: usize = sizes[keep..end].iter().sum();
        if used + row_size > room {
            break;
        }
        used += row_size;
        keep = end;
    }
    if keep == 0 || keep >= total {
        return None;
    }
    let rest: Vec<(String, u32)> = items[keep..]
        .iter()
        .filter_map(|x| x.get("well").and_then(Value::as_str))
        .map(|w| {
            let (letters, col) = split_well(w);
            (letters.to_string(), col)
        })
        .collect();
    let well = |x: &Value| x.get("well").and_then(Value::as_str).map(str::to_string);
    let first = well(items.first()?)?;
    let last = well(&items[keep - 1])?;
    let (from_row, to_row) = (rest.first()?.0.clone(), rest.last()?.0.clone());
    let from_col = rest.iter().map(|x| x.1).min()?;
    let to_col = rest.iter().map(|x| x.1).max()?;
    items.truncate(keep);
    Some(format!(
        "{key}: wells {first}–{last} here ({keep} of {total} rows, whole plate rows); the rest: wells = \"{from_row}{from_col}:{to_row}{to_col}\""
    ))
}

/// `"AB12"` → `("AB", 12)`.
fn split_well(w: &str) -> (&str, u32) {
    let i = w.find(|c: char| c.is_ascii_digit()).unwrap_or(w.len());
    (&w[..i], w[i..].parse().unwrap_or(0))
}

/// Relax an output schema so that the rows of `rows` may leave out any field: the default view
/// does. `schema` is a tool's output schema with its `$defs`.
pub fn relax_schema(schema: &mut Map<String, Value>, rows: &[Rows]) {
    let names: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let items = schema.get("properties")?.get(r.key)?.get("items")?;
            let target = items.get("$ref")?.as_str()?;
            target.rsplit('/').next().map(str::to_string)
        })
        .collect();
    let Some(Value::Object(defs)) = schema.get_mut("$defs") else {
        return;
    };
    for n in names {
        if let Some(Value::Object(d)) = defs.get_mut(&n) {
            d.remove("required");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn growth_row(w: &str) -> Value {
        json!({"well": w, "role": "unassigned", "group": w, "n_points": 559, "window": 56,
               "threshold": 0.012_345_6, "pattern": "grew", "growth_rate_per_h": 0.812_345_678,
               "doubling_time_h": 0.853, "doubling_time_min": 51.2, "max_value": 1.23456,
               "time_to_max_s": 26415.4, "auc_h": 7.1, "exp_phase_after_crash": false})
    }

    #[test]
    fn default_rows_are_lean_and_rounded() {
        let mut v = json!({"growth": [growth_row("A1")], "notes": []});
        let note = shape(&mut v, ASSAY, &[], &Fields::Default).unwrap();
        assert_eq!(
            v["growth"][0],
            json!({"well": "A1", "pattern": "grew", "growth_rate_per_h": 0.8123,
                   "doubling_time_h": 0.853, "max_value": 1.235, "time_to_max_s": 26415.0})
        );
        assert!(
            note.contains("growth rows") && note.contains("n_points"),
            "{note}"
        );
        let mut all = json!({"growth": [growth_row("A1")]});
        assert!(shape(&mut all, ASSAY, &[], &Fields::All).is_none());
        assert_eq!(all["growth"][0], growth_row("A1"));
        let mut some = json!({"growth": [growth_row("A1")]});
        let f = Fields::Named(vec!["auc_h".into(), "nope".into()]);
        let note = shape(&mut some, ASSAY, &[], &f).unwrap();
        assert_eq!(
            some["growth"][0],
            json!({"well": "A1", "pattern": "grew", "growth_rate_per_h": 0.8123,
                   "doubling_time_h": 0.853, "max_value": 1.235, "time_to_max_s": 26415.0,
                   "auc_h": 7.1})
        );
        assert!(note.contains("nope"), "{note}");
    }

    #[test]
    fn fields_argument() {
        let mut o = Map::new();
        assert_eq!(Fields::take(&mut o), Ok(Fields::Default));
        o.insert("fields".into(), json!(["all"]));
        assert_eq!(Fields::take(&mut o), Ok(Fields::All));
        assert!(o.is_empty());
        o.insert("fields".into(), json!("auc_h, lag_time_s"));
        assert_eq!(
            Fields::take(&mut o),
            Ok(Fields::Named(vec!["auc_h".into(), "lag_time_s".into()]))
        );
        // one string the argument coercion made a list of
        o.insert("fields".into(), json!(["run, dye", "lane"]));
        assert_eq!(
            Fields::take(&mut o),
            Ok(Fields::Named(vec![
                "run".into(),
                "dye".into(),
                "lane".into()
            ]))
        );
        o.insert("fields".into(), json!(3));
        assert!(Fields::take(&mut o).is_err());
    }

    #[test]
    fn rounding() {
        assert_eq!(round4(0.000_123_456), 0.000_123_5);
        assert_eq!(round4(32.174_038), 32.17);
        assert_eq!(round4(-1.23456), -1.235);
        assert_eq!(round4(26415.4), 26415.0);
        assert_eq!(round4(0.0), 0.0);
    }

    #[test]
    fn long_lists_page_by_plate_row() {
        let rows: Vec<Value> = ["A", "B", "C", "D"]
            .iter()
            .flat_map(|r| {
                (1..=24).map(move |c| json!({"well": format!("{r}{c}"), "x": "y".repeat(80)}))
            })
            .collect();
        let mut v = json!({"growth": rows, "notes": []});
        let max = v.to_string().len() / 2 + 400;
        let note = page_by_plate_row(&mut v, "growth", max).unwrap();
        assert_eq!(v["growth"].as_array().unwrap().len(), 48);
        assert!(v.to_string().len() <= max);
        assert!(
            note.contains("A1–B24") && note.contains("wells = \"C1:D24\""),
            "{note}"
        );
        let mut small = json!({"growth": [{"well": "A1"}]});
        assert!(page_by_plate_row(&mut small, "growth", 1000).is_none());
    }
}
