//! The model both UNICORN readers fill: curves (one trace each), event lists (logbook,
//! fractions, injections), vendor peak tables, experiment facts and the vendor tree.

use std::collections::BTreeMap;

use openreadout_core::experiment::Quantity;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::{ProvenanceMap, Source};
use serde_json::Value;

/// What a curve measures, in our vocabulary (`extra.kind` of its trace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CurveKind {
    /// UV/Vis absorbance (mAU).
    Uv,
    /// Conductivity (mS/cm).
    Conductivity,
    /// Conductivity as a percentage of a reference.
    ConductivityPercent,
    /// Concentration of eluent B (%B), set by the pumps.
    ConcentrationB,
    /// pH.
    Ph,
    /// A pressure (MPa).
    Pressure,
    /// A temperature.
    Temperature,
    /// A flow (ml/min, cm/h, CV/h).
    Flow,
    /// Anything else (UV cell path length, quaternary valve concentrations, …).
    Other,
}

impl CurveKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CurveKind::Uv => "uv",
            CurveKind::Conductivity => "conductivity",
            CurveKind::ConductivityPercent => "conductivity_percent",
            CurveKind::ConcentrationB => "concentration_b",
            CurveKind::Ph => "ph",
            CurveKind::Pressure => "pressure",
            CurveKind::Temperature => "temperature",
            CurveKind::Flow => "flow",
            CurveKind::Other => "other",
        }
    }

    /// The kind of a curve from its name and unit (both variants), refined by the UNICORN 7
    /// curve data type when there is one.
    pub(crate) fn classify(name: &str, unit: &str, vendor_type: Option<&str>) -> CurveKind {
        let n = name.to_ascii_lowercase();
        let u = unit.trim().to_ascii_lowercase();
        if u == "mau" || vendor_type == Some("UV") {
            return CurveKind::Uv;
        }
        if vendor_type == Some("pH") || n == "ph" || n.ends_with("_ph") {
            return CurveKind::Ph;
        }
        if u == "ms/cm" {
            return CurveKind::Conductivity;
        }
        if u == "%" && n.contains("cond") {
            return CurveKind::ConductivityPercent;
        }
        if u == "%b" || n == "conc b" || n == "conc" {
            return CurveKind::ConcentrationB;
        }
        if u == "mpa" || vendor_type == Some("Pressure") {
            return CurveKind::Pressure;
        }
        if u == "°c" || u == "c" || vendor_type == Some("Temperature") {
            return CurveKind::Temperature;
        }
        if u == "ml/min" || u == "cm/h" || u == "cv/h" {
            return CurveKind::Flow;
        }
        CurveKind::Other
    }
}

/// Where a curve's points are.
#[derive(Debug, Clone)]
pub(crate) enum Points {
    /// `.res`: `n` records of (int32 volume, int32 value) at `offset`; physical = stored × factor.
    Res {
        offset: u64,
        volume_factor: f64,
        value_factor: f64,
    },
    /// UNICORN 6/7 export: the outer member holding the curve's zip. An evaluated curve on a
    /// volume grid may store no volumes: `volume_grid` = (first, step) in ml then gives them.
    Zip {
        member: String,
        volume_grid: Option<(f64, f64)>,
    },
}

/// One recorded (or evaluated) curve: a trace sampled at a fixed time interval, with the
/// retention volume of every sample.
#[derive(Debug, Clone)]
pub(crate) struct Curve {
    /// Name as the file gives it (`UV 1_280`, `Cond`, `UV1_215nm`).
    pub(crate) name: String,
    pub(crate) kind: CurveKind,
    /// Unit of the values (`mAU`, `mS/cm`, `MPa`, …), as the file gives it (trimmed).
    pub(crate) unit: Option<String>,
    /// Samples.
    pub(crate) samples: u64,
    /// Time of sample 0 after the method start, minutes.
    pub(crate) start_min: f64,
    /// Sampling interval, minutes.
    pub(crate) interval_min: f64,
    /// Where the time origin came from (`Source` of `start_min`).
    pub(crate) points: Points,
    /// UV wavelength parsed from the curve name (`UV 1_280`, `UV1_215nm`).
    pub(crate) wavelength_nm: Option<f64>,
    /// Recorded by the instrument (false: an evaluated curve made by the software afterwards).
    pub(crate) original: bool,
    /// Further trace `extra` in our vocabulary.
    pub(crate) extra: BTreeMap<String, Value>,
}

/// One event (a logbook line, a fraction mark, an injection).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Event {
    /// Time after the method start, minutes.
    pub(crate) time_min: f64,
    /// Retention volume, ml.
    pub(crate) volume_ml: f64,
    /// Text: the logbook line, the fraction (tube) label, the injection number.
    pub(crate) text: String,
}

/// An event list (one table).
#[derive(Debug, Clone)]
pub(crate) struct EventList {
    /// Our table name: `logbook`, `fractions`, `injections`, or another list's vendor name.
    pub(crate) name: String,
    /// Name of the text column (`text`, `fraction`, `injection`).
    pub(crate) label: &'static str,
    pub(crate) events: Vec<Event>,
}

/// One peak of a vendor peak table (UNICORN's own integration, reported as stored).
#[derive(Debug, Clone, Default)]
pub(crate) struct Peak {
    /// Our column name → value (NaN when the file leaves it out).
    pub(crate) values: BTreeMap<&'static str, f64>,
    pub(crate) name: String,
}

/// A vendor peak table.
#[derive(Debug, Clone, Default)]
pub(crate) struct PeakTable {
    pub(crate) name: String,
    /// Trace index of the curve the peaks were integrated on, when it is one of ours.
    pub(crate) trace: Option<u32>,
    /// `volume` or `time`: the unit basis of the retention columns.
    pub(crate) basis: String,
    /// Retention unit (`ml`, `min`).
    pub(crate) retention_unit: String,
    /// Unit of heights (the curve's unit).
    pub(crate) height_unit: Option<String>,
    pub(crate) peaks: Vec<Peak>,
    pub(crate) extra: BTreeMap<String, Value>,
}

/// Peak-table columns: (our name, vendor element, unit kind: `r` retention, `h` height, `a`
/// area, `p` percent, `c` conductivity, `-` none).
pub(crate) const PEAK_COLUMNS: &[(&str, &str, char)] = &[
    ("retention", "MaxPeakRetention", 'r'),
    ("start", "StartPeakRetention", 'r'),
    ("end", "EndPeakRetention", 'r'),
    ("height", "Height", 'h'),
    ("area", "Area", 'a'),
    ("percent_of_total_area", "PercentOfTotalArea", 'p'),
    ("percent_of_total_peak_area", "PercentOfTotalPeakArea", 'p'),
    ("width", "Width", 'r'),
    ("width_at_half_height", "WidthAtHalfHeight", 'r'),
    ("resolution", "Resolution", '-'),
    ("asymmetry", "Assymetry", '-'),
    ("sigma", "Sigma", 'r'),
    ("start_endpoint_height", "StartPeakEndpointHeight", 'h'),
    ("end_endpoint_height", "EndPeakEndpointHeight", 'h'),
    ("average_conductivity", "AverageConductivity", 'c'),
];

/// A fact and the vendor field it came from.
pub(crate) type Fact = Option<(String, String)>;

/// Experiment facts, each with the vendor field it came from.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub(crate) sample_id: Fact,
    pub(crate) operator: Fact,
    pub(crate) model: Fact,
    pub(crate) serial: Fact,
    pub(crate) software_version: Fact,
    pub(crate) firmware: Fact,
    pub(crate) started_at: Option<(String, String, Source)>,
    pub(crate) ended_at: Option<(String, String, Source)>,
    pub(crate) method_name: Fact,
    /// `CHMO` id of the chromatography technique, when the method names one.
    pub(crate) technique: Option<&'static str>,
    /// Method parameters: our name → (quantity, vendor field).
    pub(crate) parameters: BTreeMap<String, (Quantity, String)>,
}

impl Facts {
    /// Set a text fact when `value` is not blank and the slot is empty.
    pub(crate) fn text(slot: &mut Fact, value: &str, from: &str) {
        let v = value.trim();
        if !v.is_empty() && slot.is_none() {
            *slot = Some((v.to_string(), from.to_string()));
        }
    }
    /// A numeric parameter in `unit`.
    pub(crate) fn number(&mut self, name: &str, v: f64, unit: &str, from: &str) {
        if v.is_finite() && !self.parameters.contains_key(name) {
            self.parameters
                .insert(name.into(), (Quantity::number(v, unit), from.into()));
        }
    }
    /// A text parameter.
    pub(crate) fn word(&mut self, name: &str, v: &str, from: &str) {
        let v = v.trim();
        if !v.is_empty() && !self.parameters.contains_key(name) {
            self.parameters
                .insert(name.into(), (Quantity::plain(v), from.into()));
        }
    }
}

/// Everything a reader learned while opening the file.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub(crate) format_version: Option<String>,
    pub(crate) curves: Vec<Curve>,
    pub(crate) events: Vec<EventList>,
    pub(crate) peak_tables: Vec<PeakTable>,
    /// Volume the retention axis is zeroed at in UNICORN's display: the last injection, ml.
    pub(crate) zero_volume_ml: Option<f64>,
    pub(crate) facts: Facts,
    pub(crate) vendor: Value,
    pub(crate) entries: Vec<LsEntry>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) notes: Vec<String>,
    pub(crate) provenance: ProvenanceMap,
}

/// UV wavelength in nm from a curve name: `UV 1_280`, `UV1_215nm`, `UV 2_254`; `None` for
/// `UV`, `UV 3_0` (a switched-off monitor channel reads 0) or anything else.
pub(crate) fn wavelength_of(name: &str) -> Option<f64> {
    let n = name.trim();
    let rest = n.strip_prefix("UV").or_else(|| n.strip_prefix("uv"))?;
    let tail = rest.rsplit('_').next()?;
    if tail.len() == rest.len() {
        return None; // no underscore
    }
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let after = &tail[digits.len()..];
    if !(after.is_empty() || after.eq_ignore_ascii_case("nm")) {
        return None;
    }
    let v: f64 = digits.parse().ok()?;
    (v > 0.0).then_some(v)
}

/// A finite number from vendor text (`.` decimal separator).
pub(crate) fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavelengths() {
        assert_eq!(wavelength_of("UV 1_280"), Some(280.0));
        assert_eq!(wavelength_of("UV1_215nm"), Some(215.0));
        assert_eq!(wavelength_of("UV 3_0"), None);
        assert_eq!(wavelength_of("UV"), None);
        assert_eq!(wavelength_of("UV 1_280_CUT_TEMP@100,BASEM"), None);
        assert_eq!(wavelength_of("Cond"), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(CurveKind::classify("UV", "mAu", None), CurveKind::Uv);
        assert_eq!(
            CurveKind::classify("% Cond", "%", Some("Conduction")),
            CurveKind::ConductivityPercent
        );
        assert_eq!(
            CurveKind::classify("Cond", "mS/cm", None),
            CurveKind::Conductivity
        );
        assert_eq!(
            CurveKind::classify("Conc B", "%", Some("Other")),
            CurveKind::ConcentrationB
        );
        assert_eq!(
            CurveKind::classify("Conc", "%B", None),
            CurveKind::ConcentrationB
        );
        assert_eq!(CurveKind::classify("pH", "", None), CurveKind::Ph);
        assert_eq!(
            CurveKind::classify("System flow (CV/h)", "CV/h", Some("Other")),
            CurveKind::Flow
        );
        assert_eq!(
            CurveKind::classify("Temp", "C", None),
            CurveKind::Temperature
        );
        assert_eq!(
            CurveKind::classify("UV cell path length", "cm", Some("Other")),
            CurveKind::Other
        );
    }
}
