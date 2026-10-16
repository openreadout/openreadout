//! Sizing of fragment-analysis runs. Files from Data Collection 3 and later (3500, SeqStudio)
//! carry the instrument software's own sizing: a size for every scan (`SMap` 2) and its peak
//! table (`Peak` 1–25). Older files carry only the traces, and the reader sizes them itself
//! against the size standard (see [`crate::sizing`]). Notes: `docs/formats/abif.md`.

use std::collections::BTreeMap;

use openreadout_core::series::{SeriesChannel, SeriesColumn, SeriesTable, json_num};
use serde_json::json;

use crate::abif::{Abif, Item};
use crate::build::Dye;
use crate::ladders::{self, Ladder};
use crate::sizing::{self, LadderMatch, Sizing};

/// Who sized the run.
#[derive(Debug, Clone)]
pub(crate) enum SizedBy {
    /// The instrument's software, whose sizes the file stores (`SVER2`, e.g. `SizeCaller v1.1.0`).
    Instrument { software: Option<String> },
    /// This reader, against the size standard the file names or the best-fitting one.
    Reader { how: &'static str },
}

/// The sizing of a run: a size for each sample of `first..=last` of the dye channels.
#[derive(Debug, Clone)]
pub(crate) struct FragmentSizing {
    pub(crate) first: usize,
    pub(crate) sizes: Vec<f64>,
    pub(crate) by: SizedBy,
    /// Column of the size standard among the dye channels.
    pub(crate) standard_column: usize,
    pub(crate) standard_dye: String,
    /// The ladder's matched fragments (the instrument's, or this reader's).
    pub(crate) ladder: Option<Sizing>,
    /// This reader's sizing against the instrument's: peaks compared, mean and largest
    /// absolute difference in bp.
    pub(crate) check: Option<(usize, f64, f64)>,
    /// The instrument's peak table.
    pub(crate) peaks: Option<SeriesTable>,
}

/// The size-standard dye: the one marked `S`, else the one named LIZ, ROX or similar, else the
/// last.
fn standard_dye(dyes: &[Dye]) -> Option<usize> {
    dyes.iter()
        .position(|d| d.standard)
        .or_else(|| {
            dyes.iter().position(|d| {
                let n = d.name.as_deref().unwrap_or("").to_ascii_uppercase();
                n.contains("LIZ") || n.contains("ROX") || n.contains("ORANGE")
            })
        })
        .or_else(|| dyes.len().checked_sub(1))
}

/// This reader's sizing of the standard's channel `y`.
fn own_sizing(y: &[f64], std_name: Option<&str>, dye_name: &str) -> Option<(Sizing, &'static str)> {
    let nz = sizing::noise(y);
    let peaks = sizing::find_peaks(y, (8.0 * nz).max(30.0), 3);
    if let Some(t) = std_name
        && let Some(l) = ladders::named(t)
    {
        let sizes = ladders::sizes_for(l, t);
        return sizing::size_with(l, &sizes, &peaks).map(|s| (s, "named in the file"));
    }
    let candidates: Vec<&'static Ladder> = ladders::for_dye(dye_name);
    sizing::best_of(&candidates, &peaks).map(|s| (s, "best fit (the file names no size standard)"))
}

/// Peak tags of one number, as numbers (`None` when absent or not numeric).
fn peak_col(a: &Abif, k: i32) -> Option<Vec<f64>> {
    a.get("Peak", k).and_then(Item::numbers)
}

/// The instrument's peak table, when the file has one: one row per peak of every dye.
fn instrument_peaks(a: &Abif, dyes: &[Dye]) -> Option<(SeriesTable, Vec<(i32, f64, f64, f64)>)> {
    let dye = peak_col(a, 1)?;
    let n = dye.len();
    let col = |k: i32| peak_col(a, k).filter(|v| v.len() == n);
    let (scan, height, size) = (col(2)?, col(7)?, col(12)?);
    let neg_nan = |v: Vec<f64>| -> Vec<f64> {
        v.into_iter()
            .map(|x| if x < 0.0 { f64::NAN } else { x })
            .collect()
    };
    let names: Vec<String> = dye
        .iter()
        .map(|d| {
            dyes.iter()
                .find(|x| f64::from(x.number) == *d)
                .and_then(|x| x.name.clone())
                .unwrap_or_else(|| format!("dye {d}"))
        })
        .collect();
    let ladder = col(21);
    let mut columns = vec![
        SeriesColumn::texts("dye", &names),
        SeriesColumn::numbers("scan", None, scan.clone()),
    ];
    if let (Some(b), Some(e)) = (col(3), col(4)) {
        columns.push(SeriesColumn::numbers("start_scan", None, b));
        columns.push(SeriesColumn::numbers("end_scan", None, e));
    }
    columns.push(SeriesColumn::numbers(
        "size",
        Some("bp"),
        neg_nan(size.clone()),
    ));
    if let (Some(b), Some(e)) = (col(13), col(14)) {
        columns.push(SeriesColumn::numbers("start_size", Some("bp"), neg_nan(b)));
        columns.push(SeriesColumn::numbers("end_size", Some("bp"), neg_nan(e)));
    }
    columns.push(SeriesColumn::numbers("height", Some("RFU"), height.clone()));
    if let Some(ar) = col(17) {
        columns.push(SeriesColumn::numbers("area", Some("RFU·bp"), ar));
    }
    if let Some(l) = &ladder {
        columns.push(SeriesColumn::numbers(
            "ladder_size",
            Some("bp"),
            l.iter()
                .map(|x| if *x > 0.0 { *x } else { f64::NAN })
                .collect(),
        ));
    }
    let mut extra = BTreeMap::new();
    extra.insert("kind".into(), json!("peaks"));
    extra.insert("sized_by".into(), json!("instrument"));
    if let Some(s) = a.text("SVER", 2) {
        extra.insert("software".into(), json!(s));
    }
    let rows: Vec<(i32, f64, f64, f64)> = (0..n)
        .map(|i| {
            (
                dye[i] as i32,
                scan[i],
                height[i],
                ladder.as_ref().map_or(0.0, |l| l[i]),
            )
        })
        .collect();
    Some((
        SeriesTable {
            name: "peaks".into(),
            columns,
            extra,
        },
        rows,
    ))
}

/// The longest run of rising, non-negative sizes in the instrument's size map.
fn sized_range(map: &[f64]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut start: Option<usize> = None;
    for i in 0..=map.len() {
        let ok = i < map.len()
            && map[i] >= 0.0
            && map[i].is_finite()
            && start.is_none_or(|_| i == 0 || map[i] > map[i - 1]);
        match (ok, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let e = i - 1;
                if best.is_none_or(|(a, b)| e - s > b - a) {
                    best = Some((s, e));
                }
                start = if i < map.len() && map[i] >= 0.0 {
                    Some(i)
                } else {
                    None
                };
            }
            _ => {}
        }
    }
    best.filter(|(a, b)| b > a)
}

/// Size a fragment run whose dye channels are `channels` (`values[c][scan]`).
pub(crate) fn size_run(
    a: &Abif,
    dyes: &[Dye],
    channels: &[SeriesChannel],
    values: &[Vec<f64>],
    notes: &mut Vec<String>,
) -> Option<FragmentSizing> {
    let k = standard_dye(dyes)?;
    let dye = &dyes[k];
    let col = channels
        .iter()
        .position(|c| c.extra.get("dye_number") == Some(&json!(dye.number)))?;
    let dye_name = channels[col].name.clone();
    let n = values[col].len();
    let std_name = a.text("StdF", 1);
    let own = own_sizing(&values[col], std_name.as_deref(), &dye_name);

    // the instrument's sizing
    if let Some(map) = a.get("SMap", 2).and_then(Item::numbers)
        && map.len() == n
        && let Some((first, last)) = sized_range(&map)
    {
        let sizes = map[first..=last].to_vec();
        let peaks = instrument_peaks(a, dyes);
        let mut ladder = None;
        if let Some((_, rows)) = &peaks {
            let matches: Vec<LadderMatch> = rows
                .iter()
                .filter(|r| r.0 == dye.number && r.3 > 0.0)
                .map(|r| LadderMatch {
                    size: r.3,
                    scan: r.1,
                    height: r.2,
                    residual: None,
                })
                .collect();
            if matches.len() >= 2 {
                let named = std_name.as_deref().and_then(ladders::named);
                let all: Vec<f64> = match (named, std_name.as_deref()) {
                    (Some(l), Some(t)) => ladders::sizes_for(l, t),
                    _ => matches.iter().map(|m| m.size).collect(),
                };
                let (short, product) = match named {
                    Some(l) => (l.name.to_string(), l.product.to_string()),
                    None => {
                        let t = std_name.clone().unwrap_or_else(|| "size standard".into());
                        (t.clone(), t)
                    }
                };
                ladder = Some(sizing::from_matches(&short, &product, &all, matches));
            }
        }
        // this reader's sizing against the instrument's, at the instrument's peaks
        let check = match (&own, &peaks) {
            (Some((s, _)), Some((_, rows))) => {
                let size_of = |scan: f64| -> Option<f64> {
                    let i = scan.round();
                    (i >= 0.0 && (i as usize) < n).then(|| map[i as usize])
                };
                let diffs: Vec<f64> = rows
                    .iter()
                    .filter_map(|r| {
                        let theirs = size_of(r.1).filter(|v| *v >= 0.0)?;
                        let ours = s.size_at(r.1.round())?;
                        Some((ours - theirs).abs())
                    })
                    .collect();
                (!diffs.is_empty()).then(|| {
                    (
                        diffs.len(),
                        diffs.iter().sum::<f64>() / diffs.len() as f64,
                        diffs.iter().copied().fold(0.0, f64::max),
                    )
                })
            }
            _ => None,
        };
        return Some(FragmentSizing {
            first,
            sizes,
            by: SizedBy::Instrument {
                software: a.text("SVER", 2),
            },
            standard_column: col,
            standard_dye: dye_name,
            ladder,
            check,
            peaks: peaks.map(|p| p.0),
        });
    }

    // this reader's sizing
    let Some((s, how)) = own else {
        notes.push(format!(
            "no size standard matched the peaks of dye {dye_name}{}; fragments are not sized",
            std_name
                .as_deref()
                .map_or(" (the file names none)".to_string(), |t| format!(
                    " (the file names {t})"
                ))
        ));
        return None;
    };
    let (lo, hi) = s.range()?;
    let first = lo.ceil().max(0.0) as usize;
    let last = (hi.floor().max(0.0) as usize).min(n.saturating_sub(1));
    if last <= first {
        return None;
    }
    let sizes: Vec<f64> = (first..=last)
        .map(|i| s.size_at(i as f64).unwrap_or(f64::NAN))
        .collect();
    if !sizes.windows(2).all(|w| w[1] > w[0]) {
        notes.push("the ladder fit does not rise steadily; fragments are not sized".into());
        return None;
    }
    Some(FragmentSizing {
        first,
        sizes,
        by: SizedBy::Reader { how },
        standard_column: col,
        standard_dye: dye_name,
        ladder: Some(s),
        check: None,
        peaks: None,
    })
}

/// The size-standard table: the ladder's matched fragments.
pub(crate) fn ladder_table(s: &Sizing, f: &FragmentSizing) -> SeriesTable {
    let mut extra = BTreeMap::new();
    extra.insert("kind".into(), json!("size_standard"));
    extra.insert("size_standard".into(), json!(s.name));
    extra.insert("product".into(), json!(s.product));
    extra.insert("dye".into(), json!(f.standard_dye));
    match &f.by {
        SizedBy::Instrument { software } => {
            extra.insert("matched_by".into(), json!("instrument"));
            if let Some(sw) = software {
                extra.insert("software".into(), json!(sw));
            }
        }
        SizedBy::Reader { how } => {
            extra.insert("matched_by".into(), json!("openreadout"));
            extra.insert("chosen".into(), json!(how));
        }
    }
    extra.insert("quality".into(), json_num(s.quality));
    if let Some(m) = s.mean_abs_residual {
        extra.insert("mean_abs_residual_bp".into(), json_num(m));
    }
    if !s.missing.is_empty() {
        extra.insert("missing_bp".into(), json!(s.missing));
    }
    SeriesTable {
        name: "size standard".into(),
        columns: vec![
            SeriesColumn::numbers(
                "size",
                Some("bp"),
                s.matches.iter().map(|m| m.size).collect(),
            ),
            SeriesColumn::numbers("scan", None, s.matches.iter().map(|m| m.scan).collect()),
            SeriesColumn::numbers(
                "height",
                Some("RFU"),
                s.matches.iter().map(|m| m.height).collect(),
            ),
            SeriesColumn::numbers(
                "residual",
                Some("bp"),
                s.matches
                    .iter()
                    .map(|m| m.residual.unwrap_or(f64::NAN))
                    .collect(),
            ),
        ],
        extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sized_range_is_the_longest_rising_run() {
        assert_eq!(
            sized_range(&[-1.0, -1.0, 0.0, 0.5, 1.0, 2.0, -1.0]),
            Some((2, 5))
        );
        assert_eq!(sized_range(&[-1.0, -1.0]), None);
        assert_eq!(sized_range(&[1.0, 2.0, 1.5, 1.6, 1.7, 1.8]), Some((2, 5)));
        assert_eq!(sized_range(&[]), None);
    }
}
