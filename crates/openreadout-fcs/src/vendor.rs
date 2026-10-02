//! Which instrument family wrote a data set, and what each parameter is (scatter, fluorescence,
//! full-spectrum detector, metal-tagged mass channel, time, …), from keywords the file itself
//! carries. Rules and the corpus files they come from: `docs/formats/fcs.md` (instrument
//! families) and `docs/provenance/fcs.md` (2026-09-23).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::file::DataSet;
use crate::gating::CompMatrix;
use crate::layout::Parameter;

/// Instrument family recognised from a data set's keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Family {
    /// BD FACSDiva software (LSR, FACSCanto, FACSAria, FACSymphony, …).
    BdFacsDiva,
    /// BD FACSChorus spectral instruments (FACSDiscover S8, …), FCS 3.2.
    BdSpectral,
    /// Cytek full-spectrum cytometers (Aurora, Northern Lights) and SpectroFlo software.
    CytekSpectral,
    /// Beckman Coulter CytoFLEX with CytExpert software.
    BeckmanCytoflex,
    /// Sony spectral analyzers (SA3800, ID7000, SP6800).
    SonySpectral,
    /// Sony cell sorters (SH800, MA900).
    SonySorter,
    /// Mass cytometers (CyTOF, Helios, XT; DVS Sciences / Fluidigm / Standard BioTools).
    MassCytometry,
}

impl Family {
    /// Manufacturer as users know it.
    pub fn vendor(self) -> &'static str {
        match self {
            Family::BdFacsDiva | Family::BdSpectral => "BD Biosciences",
            Family::CytekSpectral => "Cytek Biosciences",
            Family::BeckmanCytoflex => "Beckman Coulter",
            Family::SonySpectral | Family::SonySorter => "Sony Biotechnology",
            Family::MassCytometry => "Standard BioTools (formerly Fluidigm, DVS Sciences)",
        }
    }

    /// `conventional`, `spectral` or `mass`.
    pub fn technology(self) -> &'static str {
        match self {
            Family::CytekSpectral | Family::SonySpectral | Family::BdSpectral => "spectral",
            Family::MassCytometry => "mass",
            _ => "conventional",
        }
    }

    /// Stable id used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            Family::BdFacsDiva => "bd-facsdiva",
            Family::BdSpectral => "bd-spectral",
            Family::CytekSpectral => "cytek-spectral",
            Family::BeckmanCytoflex => "beckman-cytoflex",
            Family::SonySpectral => "sony-spectral",
            Family::SonySorter => "sony-sorter",
            Family::MassCytometry => "mass-cytometry",
        }
    }
}

fn kw<'a>(ds: &'a DataSet, k: &str) -> Option<&'a str> {
    ds.keyword(k).map(str::trim).filter(|s| !s.is_empty())
}

/// Recognise the instrument family.
pub fn family(ds: &DataSet) -> Option<Family> {
    let cyt = kw(ds, "$CYT").unwrap_or_default().to_ascii_uppercase();
    let creator = kw(ds, "CREATOR").unwrap_or_default();
    let tokens: Vec<&str> = cyt.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    if tokens
        .iter()
        .any(|t| matches!(*t, "CYTOF" | "DVSSCIENCES" | "FLUIDIGM" | "HELIOS"))
    {
        return Some(Family::MassCytometry);
    }
    if ds
        .parameters
        .iter()
        .filter(|p| metal_tag(&p.short_name).is_some())
        .count()
        >= 3
    {
        return Some(Family::MassCytometry);
    }
    if creator.starts_with("SpectroFlo")
        || cyt == "AURORA"
        || cyt.starts_with("NORTHERN LIGHTS")
        || cyt.starts_with("NL-")
    {
        return Some(Family::CytekSpectral);
    }
    if ds.keyword("CYTEXPERTFIL").is_some() || cyt.starts_with("CYTOFLEX") {
        return Some(Family::BeckmanCytoflex);
    }
    if ["SA3800", "ID7000", "SP6800"]
        .iter()
        .any(|m| cyt.starts_with(m))
    {
        return Some(Family::SonySpectral);
    }
    if ["SH800", "MA900"].iter().any(|m| cyt.starts_with(m)) {
        return Some(Family::SonySorter);
    }
    if creator.starts_with("BD FACSChorus") || ds.keyword("BDSPECTRAL UNMIXED").is_some() {
        return Some(Family::BdSpectral);
    }
    if creator.starts_with("BD FACSDiva") {
        return Some(Family::BdFacsDiva);
    }
    None
}

const ELEMENTS: &[&str] = &[
    "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S", "Cl",
    "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge", "As",
    "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In",
    "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb",
    "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg", "Tl",
    "Pb", "Bi",
];

/// A mass-cytometry channel name `<Element><mass>Di` (or `Dd`): element symbol and mass number.
pub fn metal_tag(name: &str) -> Option<(String, u32)> {
    let n = name.trim();
    let core = n.strip_suffix("Di").or_else(|| n.strip_suffix("Dd"))?;
    let letters: String = core.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits = &core[letters.len()..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mass: u32 = digits.parse().ok()?;
    if !(75..=209).contains(&mass) || !ELEMENTS.contains(&letters.as_str()) {
        return None;
    }
    Some((letters, mass))
}

/// The marker in a mass-cytometry `$PnS` like `142Nd_CD19` or `89Y_CD45` (`None` when the label
/// is only the isotope, e.g. `140Ce`).
pub fn metal_marker(label: &str, element: &str, mass: u32) -> Option<String> {
    let l = label.trim();
    let prefix = format!("{mass}{element}");
    let rest = l.strip_prefix(&prefix)?;
    let rest = rest.trim_start_matches(['_', '-', ' ']);
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Laser of a full-spectrum detector name (`UV7-A` → `UV`, `YG3-A` → `YG`).
fn spectral_laser(name: &str) -> Option<&'static str> {
    let base = name.split('-').next().unwrap_or(name);
    let letters: String = base.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits = &base[letters.len()..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    ["UV", "V", "B", "YG", "R"]
        .into_iter()
        .find(|l| *l == letters)
}

/// Role of a parameter: `time`, `scatter`, `fluorescence`, `spectral_detector`, `mass`,
/// `background`, `event_length`, `gaussian_parameter`, or `other`.
pub fn channel_kind(p: &Parameter, fam: Option<Family>) -> &'static str {
    let n = p.short_name.trim();
    let up = n.to_ascii_uppercase();
    if up == "TIME" || up.starts_with("TIME ") {
        return "time";
    }
    if up.starts_with("FSC")
        || up.starts_with("SSC")
        || up.starts_with("FS ")
        || up.starts_with("SS ")
    {
        return "scatter";
    }
    if fam == Some(Family::MassCytometry) {
        if metal_tag(n).is_some() {
            return "mass";
        }
        if up.starts_with("BCKG") {
            return "background";
        }
        if up == "EVENT_LENGTH" {
            return "event_length";
        }
        if ["CENTER", "OFFSET", "WIDTH", "RESIDUAL"].contains(&up.as_str()) {
            return "gaussian_parameter";
        }
        return "other";
    }
    if fam == Some(Family::CytekSpectral) && spectral_laser(n).is_some() {
        return "spectral_detector";
    }
    if up.starts_with("FL") || up.contains('-') || p.label.is_some() {
        return "fluorescence";
    }
    "other"
}

/// Per-column extras: `channel_kind`, `measure` (`area`/`height`/`width`), `laser` (spectral
/// detectors), `metal_tag` and `marker` (mass channels), `parameter_type` (`$PnTYPE`).
pub fn column_extras(ds: &DataSet, p: &Parameter, fam: Option<Family>) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    let kind = channel_kind(p, fam);
    m.insert("channel_kind".into(), json!(kind));
    let up = p.short_name.trim().to_ascii_uppercase();
    let measure = if up.ends_with("-A") || up.ends_with(" AREA") {
        Some("area")
    } else if up.ends_with("-H") || up.ends_with(" HEIGHT") {
        Some("height")
    } else if up.ends_with("-W") || up.ends_with(" WIDTH") {
        Some("width")
    } else {
        None
    };
    if let Some(v) = measure
        && kind != "mass"
    {
        m.insert("measure".into(), json!(v));
    }
    if kind == "spectral_detector"
        && let Some(l) = spectral_laser(&p.short_name)
    {
        m.insert("laser".into(), json!(laser_name(ds, l)));
    }
    if let Some((element, mass)) = metal_tag(&p.short_name) {
        m.insert(
            "metal_tag".into(),
            json!({"element": element, "mass": mass, "isotope": format!("{mass}{element}")}),
        );
        if let Some(marker) = p
            .label
            .as_deref()
            .and_then(|l| metal_marker(l, &element, mass))
        {
            m.insert("marker".into(), json!(marker));
        }
    }
    if fam == Some(Family::BdSpectral) {
        let unmixed = unmixed_parameters(ds);
        let feature = p.feature.as_deref().unwrap_or_default();
        let kind = if unmixed.iter().any(|u| u == p.short_name.trim()) {
            Some("unmixed_fluorescence")
        } else if !feature.is_empty() && !matches!(feature, "Area" | "Height" | "Width") {
            Some("imaging_feature")
        } else if p.detector.is_some() && kind != "scatter" && kind != "time" {
            Some("spectral_detector")
        } else {
            None
        };
        if let Some(k) = kind {
            m.insert("channel_kind".into(), json!(k));
        }
    }
    for (k, v) in [
        ("parameter_type", &p.measurement_type),
        ("detector", &p.detector),
        ("feature", &p.feature),
        ("dye", &p.tag),
        ("analyte", &p.analyte),
    ] {
        if let Some(v) = v {
            m.insert(k.into(), json!(v));
        }
    }
    m
}

/// BD FACSChorus `BDSPECTRAL UNMIXED`: the measurements that hold unmixed values.
fn unmixed_parameters(ds: &DataSet) -> Vec<String> {
    kw(ds, "BDSPECTRAL UNMIXED")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Laser name for a detector prefix: the file's own `LASERnNAME` value when one matches, else
/// the prefix spelled out.
fn laser_name(ds: &DataSet, prefix: &str) -> String {
    let wanted = match prefix {
        "UV" => "UV",
        "V" => "VIOLET",
        "B" => "BLUE",
        "YG" => "YELLOWGREEN",
        "R" => "RED",
        _ => prefix,
    };
    for i in 1..=12 {
        if let Some(n) = kw(ds, &format!("LASER{i}NAME"))
            && n.replace([' ', '-', '_'], "").to_ascii_uppercase() == wanted
        {
            return n.to_string();
        }
    }
    match prefix {
        "UV" => "UV".into(),
        "V" => "Violet".into(),
        "B" => "Blue".into(),
        "YG" => "YellowGreen".into(),
        "R" => "Red".into(),
        other => other.into(),
    }
}

fn is_identity(m: &CompMatrix) -> bool {
    m.values.iter().enumerate().all(|(i, r)| {
        r.iter().enumerate().all(|(j, v)| {
            if i == j {
                (v - 1.0).abs() < 1e-12
            } else {
                v.abs() < 1e-12
            }
        })
    })
}

/// `tables[].extra.platform`: family, vendor, technology, and what the family's keywords say.
pub fn platform(ds: &DataSet) -> Option<Value> {
    let fam = family(ds)?;
    let mut out = serde_json::Map::new();
    out.insert("family".into(), json!(fam.id()));
    out.insert("vendor".into(), json!(fam.vendor()));
    out.insert("technology".into(), json!(fam.technology()));
    if let Some(m) = kw(ds, "$CYT") {
        out.insert("model".into(), json!(m));
    }
    if let Some(s) = kw(ds, "CREATOR") {
        out.insert("software".into(), json!(s));
    } else if fam == Family::BeckmanCytoflex {
        out.insert("software".into(), json!("CytExpert"));
    }
    // Lasers (LASERnNAME / DELAY / ASF: BD FACSDiva and Cytek SpectroFlo).
    let lasers: Vec<Value> = (1..=12)
        .filter_map(|i| {
            let name = kw(ds, &format!("LASER{i}NAME"))?;
            let mut l = serde_json::Map::new();
            l.insert("name".into(), json!(name));
            if let Some(d) =
                kw(ds, &format!("LASER{i}DELAY")).and_then(crate::keywords::parse_float)
            {
                l.insert("delay".into(), json!(d));
            }
            if let Some(a) = kw(ds, &format!("LASER{i}ASF")).and_then(crate::keywords::parse_float)
            {
                l.insert("area_scaling_factor".into(), json!(a));
            }
            Some(Value::Object(l))
        })
        .collect();
    if !lasers.is_empty() {
        out.insert("lasers".into(), Value::Array(lasers));
    }
    match fam {
        Family::CytekSpectral => {
            let mut by_laser: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for p in &ds.parameters {
                if let Some(l) = spectral_laser(&p.short_name) {
                    by_laser
                        .entry(laser_name(ds, l))
                        .or_default()
                        .push(p.short_name.clone());
                }
            }
            let n: usize = by_laser.values().map(Vec::len).sum();
            if n > 0 {
                out.insert("detector_count".into(), json!(n));
                out.insert("detectors_by_laser".into(), json!(by_laser));
            }
            let raw = ds.parameters.iter().any(|p| {
                ds.keyword(&format!("$P{}TYPE", p.number))
                    .is_some_and(|t| t.contains("Raw"))
            });
            let identity = crate::events::file_matrix(ds)
                .as_ref()
                .is_some_and(is_identity);
            let unmixing = if n > 0 && (raw || identity) {
                json!({"stored": false, "note": "values are raw full-spectrum detector signals; unmixing into fluorochromes happens in SpectroFlo (or FlowJo) with reference controls and is not stored in this file"})
            } else if n == 0 {
                json!({"stored": true, "note": "parameters are fluorochromes, not detectors: the file holds unmixed values"})
            } else {
                json!({"stored": false})
            };
            out.insert("unmixing".into(), unmixing);
            for (k, name) in [
                ("USERSETTINGNAME", "user_setting"),
                ("GROUPNAME", "group"),
                ("TUBENAME", "tube"),
            ] {
                if let Some(v) = kw(ds, k) {
                    out.insert(name.into(), json!(v));
                }
            }
        }
        Family::BdSpectral => {
            let unmixed = unmixed_parameters(ds);
            let mut u = serde_json::Map::new();
            u.insert("stored".into(), json!(!unmixed.is_empty()));
            if !unmixed.is_empty() {
                u.insert("parameters".into(), json!(unmixed));
            }
            if let Some(m) = kw(ds, "BDSPECTRAL UNMIXING METHOD") {
                u.insert("method".into(), json!(m));
            }
            if let Some(a) = kw(ds, "BDSPECTRAL APPLY") {
                u.insert("applied".into(), json!(a.eq_ignore_ascii_case("TRUE")));
            }
            out.insert("unmixing".into(), Value::Object(u));
            // The full-spectrum detectors: the spillover matrix's, else the Area measurements
            // with a `$PnDET` that are not scatter or time.
            let detectors = crate::events::file_matrix(ds).map_or_else(
                || {
                    ds.parameters
                        .iter()
                        .filter(|p| {
                            p.detector.is_some()
                                && p.feature.as_deref().is_none_or(|f| f == "Area")
                                && !matches!(
                                    channel_kind(p, Some(Family::BdSpectral)),
                                    "scatter" | "time"
                                )
                        })
                        .count()
                },
                |m| m.detectors.len(),
            );
            if detectors > 0 {
                out.insert("detector_count".into(), json!(detectors));
            }
            let mut features: Vec<String> = ds
                .parameters
                .iter()
                .filter_map(|p| p.feature.clone())
                .filter(|f| !matches!(f.as_str(), "Area" | "Height" | "Width"))
                .collect();
            features.sort();
            features.dedup();
            if !features.is_empty() {
                out.insert("imaging_features".into(), json!(features));
            }
            if let Some(c) = kw(ds, "CYTOMETER CONFIGURATION NAME") {
                out.insert("cytometer_configuration".into(), json!(c));
            }
        }
        Family::BeckmanCytoflex => {
            for (k, name) in [
                ("TBNM", "tube"),
                ("CGNM", "group"),
                ("PLTNO", "plate_number"),
            ] {
                if let Some(v) = kw(ds, k) {
                    out.insert(name.into(), json!(v));
                }
            }
            if let Some(c) = kw(ds, "COMPCHH") {
                out.insert(
                    "compensation_channels".into(),
                    json!(
                        c.split('\t')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                    ),
                );
            }
        }
        Family::BdFacsDiva => {
            for (k, name) in [
                ("EXPERIMENT NAME", "experiment"),
                ("TUBE NAME", "tube"),
                ("CYTOMETER CONFIG NAME", "cytometer_configuration"),
                ("CST SETUP STATUS", "cst_setup_status"),
                ("APPLY COMPENSATION", "compensation_applied_in_acquisition"),
            ] {
                if let Some(v) = kw(ds, k) {
                    out.insert(name.into(), json!(v));
                }
            }
        }
        Family::SonySpectral | Family::SonySorter => {
            let log_display = ds
                .parameters
                .iter()
                .filter(|p| {
                    p.display
                        .as_deref()
                        .is_some_and(|d| d.starts_with("Logarithmic"))
                })
                .count();
            if log_display > 0 {
                out.insert("log_display_parameters".into(), json!(log_display));
            }
        }
        Family::MassCytometry => {
            let mut markers = serde_json::Map::new();
            let mut isotopes = Vec::new();
            for p in &ds.parameters {
                if let Some((el, mass)) = metal_tag(&p.short_name) {
                    let iso = format!("{mass}{el}");
                    if let Some(m) = p.label.as_deref().and_then(|l| metal_marker(l, &el, mass)) {
                        markers.insert(iso.clone(), json!(m));
                    }
                    isotopes.push(iso);
                }
            }
            out.insert("mass_channel_count".into(), json!(isotopes.len()));
            out.insert("isotopes".into(), json!(isotopes));
            out.insert("markers_by_isotope".into(), Value::Object(markers));
            out.insert(
                "note".into(),
                json!("values are ion counts per metal isotope (no spillover in the optical sense); arcsinh(x / 5) is the customary transform"),
            );
        }
    }
    Some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metal_tags() {
        assert_eq!(metal_tag("Nd142Di"), Some(("Nd".into(), 142)));
        assert_eq!(metal_tag("Y89Di"), Some(("Y".into(), 89)));
        assert_eq!(metal_tag("BCKG190Di"), None);
        assert_eq!(metal_tag("FL1-A"), None);
        assert_eq!(metal_tag("Xx142Di"), None);
        assert_eq!(
            metal_marker("142Nd_CD19", "Nd", 142).as_deref(),
            Some("CD19")
        );
        assert_eq!(metal_marker("140Ce", "Ce", 140), None);
        assert_eq!(
            metal_marker("167Er_CD197_CCR7", "Er", 167).as_deref(),
            Some("CD197_CCR7")
        );
    }

    #[test]
    fn spectral_lasers() {
        assert_eq!(spectral_laser("UV7-A"), Some("UV"));
        assert_eq!(spectral_laser("YG10-A"), Some("YG"));
        assert_eq!(spectral_laser("SSC-B-A"), None);
        assert_eq!(spectral_laser("FL1-A"), None);
    }
}
