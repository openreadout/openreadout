//! What the diffraction readers share: a scan (abscissa + intensities) and how scans become
//! traces.

use std::collections::BTreeMap;

use openreadout_core::assurance::Observations;
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, SeriesTrace, json_num};
use serde_json::{Value, json};

/// An axis of a goniometer in our vocabulary.
#[derive(Debug, Clone)]
pub(crate) struct ScanAxis {
    /// `two_theta`, `omega`, `theta`, `phi`, `chi`, `x`, `y`, `z`, `time`, …
    pub(crate) quantity: &'static str,
    /// `°`, `mm`, `s`.
    pub(crate) unit: Option<String>,
    /// The vendor's axis name.
    pub(crate) label: String,
}

/// Our name for a vendor axis name and unit.
pub(crate) fn axis_of(name: &str, unit: &str) -> ScanAxis {
    let n = name.trim();
    let quantity = match n.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "2theta" | "twotheta" | "2θ" | "tth" => "two_theta",
        "omega" | "ω" | "theta" | "θ" => {
            if n.eq_ignore_ascii_case("theta") || n == "θ" {
                "theta"
            } else {
                "omega"
            }
        }
        "phi" | "φ" => "phi",
        "chi" | "psi" | "χ" | "ψ" => "chi",
        "x" => "x",
        "y" => "y",
        "z" => "z",
        "time" | "t" => "time",
        "energy" => "energy",
        "q" => "q",
        _ => "angle",
    };
    let unit = match unit.trim() {
        "deg" | "degree" | "degrees" | "°" | ""
            if quantity != "x" && quantity != "y" && quantity != "z" && quantity != "time" =>
        {
            Some("°".to_string())
        }
        "" => None,
        u => Some(u.to_string()),
    };
    ScanAxis {
        quantity,
        unit,
        label: n.to_string(),
    }
}

/// One scan: the abscissa per point, the intensities and any other per-point values.
#[derive(Debug, Clone)]
pub(crate) struct Scan {
    /// Trace name (empty: named from its position and axis).
    pub(crate) name: String,
    pub(crate) axis: ScanAxis,
    pub(crate) abscissa: Vec<f64>,
    /// True when the file lists a position per point (else they are evenly spaced).
    pub(crate) listed: bool,
    pub(crate) intensity: Vec<f64>,
    pub(crate) intensity_unit: Option<String>,
    /// Further per-point channels (counting time, attenuation, other axes).
    pub(crate) extra_channels: Vec<(SeriesChannel, Vec<f64>)>,
    pub(crate) extra: BTreeMap<String, Value>,
}

/// Whitespace-separated numbers (`None` when one is not a number).
pub(crate) fn parse_numbers(s: &str) -> Option<Vec<f64>> {
    s.split_whitespace()
        .map(|t| t.parse::<f64>().ok())
        .collect()
}

/// True when `v` is evenly spaced within a relative tolerance of the step.
fn evenly_spaced(v: &[f64]) -> Option<(f64, f64)> {
    if v.len() < 2 {
        return v.first().map(|f| (*f, 0.0));
    }
    let n = v.len();
    let step = (v[n - 1] - v[0]) / (n - 1) as f64;
    if step == 0.0 || !step.is_finite() {
        return None;
    }
    let tol = step.abs() * 1e-6 + 1e-9;
    v.iter()
        .enumerate()
        .all(|(i, x)| (x - (v[0] + step * i as f64)).abs() <= tol)
        .then_some((v[0], step))
}

/// What a reader hands to [`finish`] besides the scans and the experiment facts.
pub(crate) struct FileParts {
    pub(crate) format_version: Option<String>,
    pub(crate) vendor: Value,
    pub(crate) entries: Vec<LsEntry>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) observations: Observations,
    pub(crate) check: &'static str,
}

/// Build the file from its scans: one trace per scan (channel `intensity`, plus the abscissa as
/// channel 0 when the file lists positions, plus the scan's extra channels).
pub(crate) fn finish(scans: Vec<Scan>, mut facts: Facts, parts: FileParts) -> SeriesFile {
    let FileParts {
        format_version,
        vendor,
        entries,
        findings,
        observations,
        check,
    } = parts;
    let many = scans.len() > 1;
    let mut traces = Vec::new();
    let mut total = 0usize;
    for (k, s) in scans.into_iter().enumerate() {
        let n = s.intensity.len();
        total += n;
        let mut channels = Vec::new();
        let mut values = Vec::new();
        let mut extra = s.extra;
        let regular = if s.listed {
            None
        } else {
            evenly_spaced(&s.abscissa)
        };
        if let Some((first, step)) = regular {
            {
                let mut ax = json!({
                    "quantity": s.axis.quantity,
                    "first": json_num(first),
                    "step": json_num(step),
                    "last": json_num(first + step * n.saturating_sub(1) as f64),
                    "size": n,
                });
                if let Some(u) = &s.axis.unit {
                    ax["unit"] = json!(u);
                }
                if !s.axis.label.is_empty() {
                    ax["label"] = json!(s.axis.label);
                }
                extra.insert("axis".into(), ax);
            }
        } else {
            {
                channels.push(SeriesChannel::new(
                    s.axis.quantity,
                    s.axis.unit.as_deref(),
                    "float64",
                ));
                values.push(s.abscissa);
                let mut ax = json!({"quantity": s.axis.quantity, "irregular": true, "channel": 0, "size": n});
                if let Some(u) = &s.axis.unit {
                    ax["unit"] = json!(u);
                }
                if !s.axis.label.is_empty() {
                    ax["label"] = json!(s.axis.label);
                }
                extra.insert("axis".into(), ax);
            }
        }
        channels.push(SeriesChannel::new(
            "intensity",
            s.intensity_unit.as_deref(),
            "float64",
        ));
        values.push(s.intensity);
        for (c, v) in s.extra_channels {
            channels.push(c);
            values.push(v);
        }
        extra.insert("kind".into(), json!("diffractogram"));
        extra.insert("data_type".into(), json!("X-RAY DIFFRACTION"));
        let name = if !s.name.is_empty() {
            s.name
        } else if many {
            format!("scan {} ({})", k + 1, s.axis.label)
        } else {
            format!("{} scan", s.axis.label)
        };
        traces.push(SeriesTrace {
            name,
            channels,
            sweeps: vec![values],
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }
    facts.technique("CHMO:0000156", "the file format (X-ray diffraction scans)");
    facts.measurement(
        MeasurementKind::Trace,
        (0..traces.len() as u32).collect(),
        format!(
            "X-ray diffraction, {} scan{}, {} points",
            traces.len(),
            if traces.len() == 1 { "" } else { "s" },
            total
        ),
        Some("CHMO:0000156"),
    );
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::Spec);
    SeriesFile {
        format_version,
        traces,
        tables: Vec::new(),
        experiment: Some(facts.build()),
        vendor,
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec![check.to_string()],
        members: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_and_spacing() {
        assert_eq!(axis_of("2Theta", "deg").quantity, "two_theta");
        assert_eq!(axis_of("2Theta", "deg").unit.as_deref(), Some("°"));
        assert_eq!(axis_of("Omega", "deg").quantity, "omega");
        assert_eq!(axis_of("X", "mm").unit.as_deref(), Some("mm"));
        assert_eq!(evenly_spaced(&[1.0, 2.0, 3.0]), Some((1.0, 1.0)));
        assert_eq!(evenly_spaced(&[1.0, 2.0, 3.5]), None);
        assert_eq!(parse_numbers("1 2  3.5\n4"), Some(vec![1.0, 2.0, 3.5, 4.0]));
        assert_eq!(parse_numbers("1 x"), None);
    }
}
