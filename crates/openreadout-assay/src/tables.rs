//! Tidy CSV tables of a result (`--csv PREFIX`): `PREFIX.wells.csv`, `.samples.csv`,
//! `.compounds.csv`, `.kinetics.csv`, `.growth.csv`, `.curve.csv` (the fitted points), and a
//! human-readable summary. Every file is written to a temporary name, read back, compared and
//! renamed into place.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use openreadout_core::{Error, Result};
use serde::Serialize;
use serde_json::Value;

use crate::output::AssayOutput;

fn cell(v: &Value) -> String {
    let s = match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string))
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(_) => v.to_string(),
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Rows as CSV: the union of the rows' keys in first-seen order; nested `extra` maps are
/// flattened to `extra.<key>`.
pub fn to_csv<T: Serialize>(rows: &[T]) -> Result<String> {
    let mut flat: Vec<Vec<(String, Value)>> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for r in rows {
        let v = serde_json::to_value(r).map_err(|e| Error::Other(e.to_string()))?;
        let Value::Object(m) = v else { continue };
        let mut row = Vec::new();
        for (k, v) in m {
            if let (true, Value::Object(inner)) = (k == "extra", &v) {
                for (ik, iv) in inner {
                    row.push((format!("extra.{ik}"), iv.clone()));
                }
            } else {
                row.push((k, v));
            }
        }
        for (k, _) in &row {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        flat.push(row);
    }
    let mut out = keys
        .iter()
        .map(|k| cell(&Value::String(k.clone())))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for row in &flat {
        let line: Vec<String> = keys
            .iter()
            .map(|k| {
                row.iter()
                    .find(|(rk, _)| rk == k)
                    .map_or_else(String::new, |(_, v)| cell(v))
            })
            .collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    Ok(out)
}

/// Write `bytes` to `path` via a temporary file, read it back, compare, rename.
pub fn write_verified(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if path.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            path.display()
        )));
    }
    let name = path
        .file_name()
        .map_or_else(|| "out".into(), |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(&tmp, e))?;
    let back = std::fs::read(&tmp).map_err(|e| Error::io(&tmp, e))?;
    if back != bytes {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Other(format!(
            "{}: read-back differs from what was written",
            tmp.display()
        )));
    }
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
}

/// Write the tidy tables of `out` under `prefix`; returns the paths written.
pub fn write_tables(out: &AssayOutput, prefix: &Path, overwrite: bool) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let base = prefix.to_string_lossy().into_owned();
    let mut put = |suffix: &str, text: String| -> Result<()> {
        let p = PathBuf::from(format!("{base}.{suffix}.csv"));
        write_verified(&p, text.as_bytes(), overwrite)?;
        written.push(p);
        Ok(())
    };
    if !out.wells.is_empty() {
        put("wells", to_csv(&out.wells)?)?;
    }
    if !out.samples.is_empty() {
        put("samples", to_csv(&out.samples)?)?;
    }
    if !out.compounds.is_empty() {
        #[derive(Serialize)]
        struct Row<'a> {
            compound: &'a str,
            kind: &'a str,
            ec50: Option<f64>,
            ec50_se: Option<f64>,
            ec50_ci_low: Option<f64>,
            ec50_ci_high: Option<f64>,
            absolute_ec50: Option<f64>,
            hill_slope: Option<f64>,
            top: Option<f64>,
            bottom: Option<f64>,
            r_squared: Option<f64>,
            n_concentrations: usize,
            extrapolated: bool,
            error: Option<&'a str>,
        }
        let rows: Vec<Row> = out
            .compounds
            .iter()
            .map(|c| Row {
                compound: &c.compound,
                kind: &c.kind,
                ec50: c.ec50,
                ec50_se: c.ec50_se,
                ec50_ci_low: c.ec50_ci_low,
                ec50_ci_high: c.ec50_ci_high,
                absolute_ec50: c.absolute_ec50,
                hill_slope: c.hill_slope,
                top: c.top,
                bottom: c.bottom,
                r_squared: c.fit.as_ref().map(|f| f.r_squared),
                n_concentrations: c.n_concentrations,
                extrapolated: c.extrapolated,
                error: c.error.as_deref(),
            })
            .collect();
        put("compounds", to_csv(&rows)?)?;
    }
    if !out.kinetics.is_empty() {
        put("kinetics", to_csv(&out.kinetics)?)?;
    }
    if !out.growth.is_empty() {
        put("growth", to_csv(&out.growth)?)?;
    }
    if let Some(c) = &out.curve {
        put("curve", to_csv(&c.fit.points)?)?;
    }
    Ok(written)
}

fn num(v: Option<f64>) -> String {
    v.map_or_else(
        || "-".into(),
        |x| {
            format!("{x:.6}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        },
    )
}

/// A human-readable summary of a result.
pub fn render_text(o: &AssayOutput) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{} — {} of read {} `{}`{}{}",
        o.path,
        o.analysis,
        o.read.number,
        o.read.label,
        o.read
            .unit
            .as_ref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default(),
        o.reduce
            .as_ref()
            .map(|r| format!(", reduced by {r}"))
            .unwrap_or_default()
    );
    let roles: Vec<String> = o
        .layout
        .roles
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    let _ = writeln!(
        s,
        "layout: {} ({})",
        if o.layout.sources.is_empty() {
            "none".into()
        } else {
            o.layout.sources.join(", ")
        },
        roles.join(", ")
    );
    if let Some(b) = &o.blank {
        let _ = writeln!(
            s,
            "blank: {} of {} wells = {}{}",
            b.method,
            b.n,
            num(Some(b.value)),
            if b.per_time_point {
                " (per time point)"
            } else {
                ""
            }
        );
    }
    if !o.outliers.flagged.is_empty() {
        let _ = writeln!(
            s,
            "outliers ({}): {}{}",
            o.outliers.rule,
            o.outliers.flagged.join(", "),
            if o.outliers.excluded {
                " (excluded)"
            } else {
                " (flagged, kept)"
            }
        );
    }
    if let Some(c) = &o.curve {
        let f = &c.fit;
        let _ = writeln!(
            s,
            "\nstandard curve: {} [{}], fit on {}, weighting {}",
            f.model, f.formula, c.fit_on, f.weighting
        );
        for p in &f.parameters {
            let _ = writeln!(
                s,
                "  {:<9} {:>14}  SE {:>12}  CI [{}, {}]",
                p.name,
                num(Some(p.value)),
                num(p.se),
                num(p.ci_low),
                num(p.ci_high)
            );
        }
        let _ = writeln!(
            s,
            "  R² {:.5}, n {}, levels {}, range {}–{}, LLOQ {}, ULOQ {}",
            f.r_squared,
            f.n_points,
            f.n_levels,
            num(Some(c.range_low)),
            num(Some(c.range_high)),
            num(c.lloq),
            num(c.uloq)
        );
    }
    for c in &o.compounds {
        match &c.error {
            Some(e) if c.ec50.is_none() => {
                let _ = writeln!(s, "{}: {e}", c.compound);
            }
            _ => {
                let _ = writeln!(
                    s,
                    "{}: {} {} (95% CI {}–{}), Hill {}, top {}, bottom {}{}",
                    c.compound,
                    c.kind,
                    num(c.ec50),
                    num(c.ec50_ci_low),
                    num(c.ec50_ci_high),
                    num(c.hill_slope),
                    num(c.top),
                    num(c.bottom),
                    if c.extrapolated {
                        " (extrapolated)"
                    } else {
                        ""
                    }
                );
            }
        }
    }
    if !o.samples.is_empty() && o.kinetics.is_empty() && o.growth.is_empty() {
        let _ = writeln!(
            s,
            "\n{:<22} {:<10} {:>4} {:>12} {:>10} {:>7} {:>12} {:>12}",
            "group", "role", "n", "mean", "sd", "cv%", "conc", "flag"
        );
        for r in &o.samples {
            let _ = writeln!(
                s,
                "{:<22} {:<10} {:>4} {:>12} {:>10} {:>7} {:>12} {:>12}",
                r.group,
                r.role,
                r.n_used,
                num(r.mean),
                num(r.sd),
                r.cv_percent
                    .map_or_else(|| "-".into(), |c| format!("{c:.1}")),
                num(r.final_concentration.or(r.back_calculated_mean)),
                r.flag.clone().unwrap_or_default()
            );
        }
    }
    if !o.kinetics.is_empty() {
        let _ = writeln!(
            s,
            "\n{:<6} {:>14} {:>10} {:>12} {:>12}",
            "well", "max slope/min", "r²", "lag (s)", "t max (s)"
        );
        for k in &o.kinetics {
            let _ = writeln!(
                s,
                "{:<6} {:>14} {:>10.4} {:>12} {:>12}",
                k.well,
                num(Some(k.max_slope_per_min)),
                k.max_slope_r_squared,
                num(k.lag_time_s),
                num(Some(k.time_to_max_s))
            );
        }
    }
    if !o.growth.is_empty() {
        let _ = writeln!(
            s,
            "\n{:<6} {:>10} {:>14} {:>12} {:>16}",
            "well", "µmax /h", "doubling (min)", "lag (s)", "logistic r /h"
        );
        for g in &o.growth {
            let _ = writeln!(
                s,
                "{:<6} {:>10} {:>14} {:>12} {:>16}",
                g.well,
                num(g.growth_rate_per_h),
                num(g.doubling_time_min),
                num(g.lag_time_s),
                num(g.logistic_r_per_h)
            );
        }
    }
    if let Some(q) = &o.quality {
        let _ = writeln!(
            s,
            "\nquality: Z′ {} ({}), S/B {}, S/N {}, SSMD {}, median replicate CV {}%",
            num(q.z_prime),
            q.assessment.clone().unwrap_or_else(|| "-".into()),
            num(q.signal_to_background),
            num(q.signal_to_noise),
            num(q.ssmd),
            num(q.median_replicate_cv_percent)
        );
    }
    for w in &o.written {
        let _ = writeln!(s, "wrote {w}");
    }
    for n in &o.notes {
        let _ = writeln!(s, "note: {n}");
    }
    for w in &o.warnings {
        let _ = writeln!(s, "warning: {w}");
    }
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_and_flattens() {
        #[derive(Serialize)]
        struct R {
            a: String,
            b: Option<f64>,
            extra: std::collections::BTreeMap<String, String>,
        }
        let mut e = std::collections::BTreeMap::new();
        e.insert("note".to_string(), "x, y".to_string());
        let t = to_csv(&[R {
            a: "q\"t".into(),
            b: None,
            extra: e,
        }])
        .unwrap();
        assert_eq!(t, "a,b,extra.note\n\"q\"\"t\",,\"x, y\"\n");
    }

    #[test]
    fn verified_write() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.csv");
        write_verified(&p, b"a\n", false).unwrap();
        assert!(write_verified(&p, b"b\n", false).is_err());
        write_verified(&p, b"b\n", true).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"b\n");
    }
}
