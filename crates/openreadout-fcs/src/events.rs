//! From raw DATA values to the values analysis works on: scale values (FCS 3.1 §3.2.19–20),
//! the file's own compensation matrix, and parameter roles.

use crate::dataset::spillover;
use crate::file::DataSet;
use crate::gating::CompMatrix;

/// True for the time parameter (`$PnN` `Time`, any case).
pub fn is_time_parameter(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("time")
}

/// True for fluorescence (or mass) parameters: not scatter (`FSC-…`, `SSC-…`) and not time,
/// the split FlowIO also makes. Used as the default set for `--transform`.
pub fn is_fluorescence_parameter(name: &str) -> bool {
    let l = name.trim().to_ascii_lowercase();
    !(l.starts_with("fsc") || l.starts_with("ssc") || l == "time")
}

/// Convert raw DATA values to scale values in place: `$PnE f1,f2` with `f1 > 0` gives
/// `10^(f1·x/$PnR)·f2` (`f2 = 0` read as 1); `$PnG` other than 0 or 1 divides; the time
/// parameter is multiplied by `$TIMESTEP` and never divided by a gain. Returns what was done,
/// one entry per changed parameter.
pub fn scale_columns(ds: &DataSet, columns: &mut [Vec<f64>]) -> Vec<String> {
    let timestep = ds
        .keyword("$TIMESTEP")
        .and_then(crate::keywords::parse_float)
        .filter(|t| *t != 0.0);
    let mut done = Vec::new();
    for (p, col) in ds.parameters.iter().zip(columns.iter_mut()) {
        let time = is_time_parameter(&p.short_name);
        if let Some([decades, offset]) = p.amplification
            && decades > 0.0
            && let Some(range) = p.range.filter(|r| *r > 0.0)
        {
            let offset = if offset == 0.0 { 1.0 } else { offset };
            for v in col.iter_mut() {
                *v = 10f64.powf(decades * *v / range) * offset;
            }
            done.push(format!(
                "{}: log amplification $PnE {decades},{offset}",
                p.short_name
            ));
        }
        if time {
            if let Some(t) = timestep {
                for v in col.iter_mut() {
                    *v *= t;
                }
                done.push(format!("{}: × $TIMESTEP {t}", p.short_name));
            }
        } else if let Some(g) = p.gain
            && g != 0.0
            && (g - 1.0).abs() > f64::EPSILON
        {
            for v in col.iter_mut() {
                *v /= g;
            }
            done.push(format!("{}: ÷ $PnG {g}", p.short_name));
        }
    }
    done
}

/// The file's own compensation matrix: `$SPILLOVER`, `$SPILL` or `SPILL` (the latter only when
/// its names are this data set's parameters). `$COMP` is not used: FCS 3.0 does not say
/// whether it holds spillover or compensation coefficients.
pub fn file_matrix(ds: &DataSet) -> Option<CompMatrix> {
    let m = spillover(ds)?;
    if m.keyword == "$COMP" {
        return None;
    }
    CompMatrix::from_spillover(&m)
}

/// `$PnN` names and `$PnS` labels of a data set.
pub fn parameter_names(ds: &DataSet) -> (Vec<String>, Vec<Option<String>>) {
    (
        ds.parameters.iter().map(|p| p.short_name.clone()).collect(),
        ds.parameters.iter().map(|p| p.label.clone()).collect(),
    )
}
