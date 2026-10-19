//! QuantaSoft `.qlp` plates against QuantaSoft's results export of the same plate
//! (`oracle/qlp_oracle.py`): per exported well and target, the sample and target, the accepted,
//! positive and negative droplets and the four double-channel counts tallied from our droplet
//! classes, and the mean amplitudes of the positive and negative droplets computed from our
//! droplets table (to the precision the export printed). A well the export calls `No Call` may
//! have no counts in ours (its classes are not stored).

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Equal to `printed` to the precision the export printed: half a unit of its last decimal
/// (1 % more, because QuantaSoft averages in single precision and a mean that ends in 5 can
/// round either way), and at least 1e-6 relative.
fn as_printed(ours: f64, printed: f64) -> bool {
    let text = format!("{printed}");
    let decimals = text.split_once('.').map_or(0, |(_, d)| d.len());
    let half = 0.505 * 10f64.powi(-(decimals as i32));
    (ours - printed).abs() <= half.max(1e-6 * printed.abs())
}

pub fn compare(path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_benchtop::QlpReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let wt = info.tables.first().ok_or("no wells table")?;
    let dt = info.tables.get(1).ok_or("no droplets table")?;
    let w = ds
        .read_table(0, 0, u64::MAX)
        .map_err(|e| format!("read failed: {e}"))?;
    let d = ds
        .read_table(1, 0, u64::MAX)
        .map_err(|e| format!("read failed: {e}"))?;
    let col = |name: &str| wt.columns.iter().position(|c| c.name == name);
    let cats = |c: &openreadout_core::ColumnInfo| -> Vec<String> {
        c.extra
            .get("categories")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let text = |name: &str, i: usize| -> String {
        col(name)
            .and_then(|k| cats(&wt.columns[k]).get(w.columns[k][i] as usize).cloned())
            .unwrap_or_default()
    };
    let num = |name: &str, i: usize| col(name).map_or(f64::NAN, |k| w.columns[k][i]);
    let well_names = cats(&dt.columns[0]);
    let mut problems = Vec::new();
    let mut compared = 0usize;
    let mut no_call = 0usize;
    let empty = Vec::new();
    for e in oracle["wells"].as_array().unwrap_or(&empty) {
        let well = e["well"].as_str().unwrap_or_default();
        let tt = e["target_type"].as_str().unwrap_or_default();
        let ch = usize::from(tt.starts_with("Ch2"));
        let Some(i) = (0..wt.row_count as usize)
            .find(|&i| text("well", i) == well && text("channel", i) == format!("Ch{}", ch + 1))
        else {
            problems.push(format!("{well} {tt}: no row"));
            continue;
        };
        if text("target", i) != e["target"].as_str().unwrap_or_default().trim()
            || text("sample", i) != e["sample"].as_str().unwrap_or_default().trim()
            || !tt.ends_with(&text("target_type", i))
        {
            problems.push(format!(
                "{well} {tt}: sample/target/type {:?}/{:?}/{:?}, export {}/{}/{tt}",
                text("sample", i),
                text("target", i),
                text("target_type", i),
                e["sample"],
                e["target"]
            ));
            continue;
        }
        if num("accepted_droplets", i).is_nan() {
            if e["concentration"].as_str() == Some("No Call") {
                no_call += 1;
                continue;
            }
            problems.push(format!(
                "{well} {tt}: no droplet counts in ours, export {}",
                e["accepted"]
            ));
            continue;
        }
        for (ours, theirs) in [
            ("accepted_droplets", "accepted"),
            ("positive_droplets", "positives"),
            ("negative_droplets", "negatives"),
            ("droplets_ch1_pos_ch2_pos", "ch1_pos_ch2_pos"),
            ("droplets_ch1_pos_ch2_neg", "ch1_pos_ch2_neg"),
            ("droplets_ch1_neg_ch2_pos", "ch1_neg_ch2_pos"),
            ("droplets_ch1_neg_ch2_neg", "ch1_neg_ch2_neg"),
        ] {
            if let Some(v) = e[theirs].as_f64()
                && num(ours, i).to_bits() != v.to_bits()
            {
                problems.push(format!("{well} {tt}: {ours} {} vs export {v}", num(ours, i)));
            }
        }
        let Some(wi) = well_names.iter().position(|n| n == well) else {
            problems.push(format!("{well}: no droplets"));
            continue;
        };
        let amp = if ch == 0 { 2 } else { 3 };
        let (mut sp, mut np, mut sn, mut nn) = (0.0, 0usize, 0.0, 0usize);
        for k in 0..d.columns[0].len() {
            if d.columns[0][k] as usize != wi {
                continue;
            }
            let c = d.columns[5][k];
            if c.is_nan() || c == 0.0 {
                continue;
            }
            let pos = if ch == 0 {
                c == 2.0 || c == 3.0
            } else {
                c == 3.0 || c == 4.0
            };
            if pos {
                sp += d.columns[amp][k];
                np += 1;
            } else {
                sn += d.columns[amp][k];
                nn += 1;
            }
        }
        for (sum, n, key) in [
            (sp, np, "mean_amplitude_positives"),
            (sn, nn, "mean_amplitude_negatives"),
        ] {
            if let Some(v) = e[key].as_f64()
                && n > 0
                && !as_printed(sum / n as f64, v)
            {
                problems.push(format!("{well} {tt}: {key} {} vs export {v}", sum / n as f64));
            }
        }
        compared += 1;
    }
    if compared == 0 {
        problems.push("no well compared".into());
    }
    if problems.is_empty() {
        Ok(format!(
            "{compared} exported wells × targets: droplet counts and mean amplitudes equal QuantaSoft's export{}",
            if no_call > 0 {
                format!("; {no_call} No Call well(s) without stored classes not compared")
            } else {
                String::new()
            }
        ))
    } else {
        let n = problems.len();
        problems.truncate(8);
        Err(format!("{n} problem(s): {}", problems.join("; ")))
    }
}
