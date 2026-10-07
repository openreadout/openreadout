//! PSI-MS controlled-vocabulary terms the mzML reader interprets. Accessions and names were
//! checked against `psi-ms.obo` 4.2.2 (see `docs/provenance/mzml.md`).

use std::collections::HashMap;

use crate::binary::{Compression, Outer, ValueType};
use crate::xml::Node;

/// One `cvParam` or `userParam` (user params have an empty accession).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Param {
    pub(crate) accession: String,
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) unit_accession: String,
    pub(crate) unit_name: String,
}

impl Param {
    pub(crate) fn f64(&self) -> Option<f64> {
        self.value
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())
    }
    /// The term's meaning for a summary: its name, or for a generic parent term written with
    /// the specific value in `value` (`software` = "Xcalibur", `detector type` =
    /// "electron multiplier", as some writers do when no specific term fits) that value.
    pub(crate) fn label(&self) -> String {
        if GENERIC_PARENTS.contains(&self.accession.as_str()) && !self.value.trim().is_empty() {
            self.value.trim().to_string()
        } else {
            self.name.clone()
        }
    }
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        if !self.accession.is_empty() {
            m.insert("accession".into(), self.accession.clone().into());
        }
        m.insert("name".into(), self.name.clone().into());
        if !self.value.is_empty() {
            m.insert("value".into(), self.value.clone().into());
        }
        if !self.unit_name.is_empty() || !self.unit_accession.is_empty() {
            m.insert(
                "unit".into(),
                if self.unit_name.is_empty() {
                    self.unit_accession.clone()
                } else {
                    self.unit_name.clone()
                }
                .into(),
            );
        }
        serde_json::Value::Object(m)
    }
}

/// Parent terms that writers fill with a free-text `value` when no specific child term fits:
/// instrument model, software, ionization type, mass analyzer type, detector type.
pub(crate) const GENERIC_PARENTS: [&str; 5] = [
    "MS:1000031",
    "MS:1000531",
    "MS:1000008",
    "MS:1000443",
    "MS:1000026",
];

/// `referenceableParamGroup` id → its params.
pub(crate) type ParamGroups = HashMap<String, Vec<Param>>;

fn param_of(n: &Node) -> Option<Param> {
    match n.tag.as_str() {
        "cvParam" | "userParam" => Some(Param {
            accession: n.attr("accession").unwrap_or_default().to_string(),
            name: n.attr("name").unwrap_or_default().to_string(),
            value: n.attr("value").unwrap_or_default().to_string(),
            unit_accession: n.attr("unitAccession").unwrap_or_default().to_string(),
            unit_name: n.attr("unitName").unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

/// The params directly under `n`, with `referenceableParamGroupRef`s expanded in place.
pub(crate) fn params(n: &Node, groups: &ParamGroups) -> Vec<Param> {
    let mut out = Vec::new();
    for c in &n.children {
        if c.tag == "referenceableParamGroupRef" {
            if let Some(g) = c.attr("ref").and_then(|r| groups.get(r)) {
                out.extend(g.iter().cloned());
            }
        } else if let Some(p) = param_of(c) {
            out.push(p);
        }
    }
    out
}

/// Read `referenceableParamGroupList`.
pub(crate) fn param_groups(list: Option<&Node>) -> ParamGroups {
    let mut g = ParamGroups::new();
    if let Some(list) = list {
        for grp in list.children_named("referenceableParamGroup") {
            if let Some(id) = grp.attr("id") {
                g.insert(
                    id.to_string(),
                    grp.children.iter().filter_map(param_of).collect(),
                );
            }
        }
    }
    g
}

pub(crate) fn find<'a>(ps: &'a [Param], accession: &str) -> Option<&'a Param> {
    ps.iter().find(|p| p.accession == accession)
}

pub(crate) fn has(ps: &[Param], accession: &str) -> bool {
    find(ps, accession).is_some()
}

pub(crate) fn value_f64(ps: &[Param], accession: &str) -> Option<f64> {
    find(ps, accession).and_then(Param::f64)
}

// spectrum
pub(crate) const MS_LEVEL: &str = "MS:1000511";
pub(crate) const MS1_SPECTRUM: &str = "MS:1000579";
pub(crate) const EM_SPECTRUM: &str = "MS:1000804";
pub(crate) const CENTROID: &str = "MS:1000127";
pub(crate) const NEGATIVE: &str = "MS:1000129";
pub(crate) const POSITIVE: &str = "MS:1000130";
pub(crate) const TIC: &str = "MS:1000285";
pub(crate) const BASE_PEAK_MZ: &str = "MS:1000504";
pub(crate) const BASE_PEAK_INTENSITY: &str = "MS:1000505";
pub(crate) const HIGHEST_MZ: &str = "MS:1000527";
pub(crate) const LOWEST_MZ: &str = "MS:1000528";
// scan
pub(crate) const SCAN_START_TIME: &str = "MS:1000016";
pub(crate) const FILTER_STRING: &str = "MS:1000512";
pub(crate) const ION_INJECTION_TIME: &str = "MS:1000927";
pub(crate) const SCAN_WINDOW_LOWER: &str = "MS:1000501";
pub(crate) const SCAN_WINDOW_UPPER: &str = "MS:1000500";
pub(crate) const INVERSE_REDUCED_MOBILITY: &str = "MS:1002815";
// precursor
pub(crate) const SELECTED_ION_MZ: &str = "MS:1000744";
pub(crate) const CHARGE_STATE: &str = "MS:1000041";
pub(crate) const PEAK_INTENSITY: &str = "MS:1000042";
pub(crate) const ISOLATION_TARGET: &str = "MS:1000827";
pub(crate) const ISOLATION_LOWER_OFFSET: &str = "MS:1000828";
pub(crate) const ISOLATION_UPPER_OFFSET: &str = "MS:1000829";
pub(crate) const COLLISION_ENERGY: &str = "MS:1000045";
pub(crate) const NORMALIZED_COLLISION_ENERGY: &str = "MS:1000138";
// arrays
pub(crate) const MZ_ARRAY: &str = "MS:1000514";
pub(crate) const INTENSITY_ARRAY: &str = "MS:1000515";
pub(crate) const TIME_ARRAY: &str = "MS:1000595";
pub(crate) const NON_STANDARD_ARRAY: &str = "MS:1000786";
// chromatograms
pub(crate) const TIC_CHROMATOGRAM: &str = "MS:1000235";
pub(crate) const BPC_CHROMATOGRAM: &str = "MS:1000628";
// imzML (imagingMS.obo 1.1.0)
pub(crate) const CONTINUOUS: &str = "IMS:1000030";
pub(crate) const PROCESSED: &str = "IMS:1000031";
pub(crate) const PIXELS_X: &str = "IMS:1000042";
pub(crate) const PIXELS_Y: &str = "IMS:1000043";
pub(crate) const DIMENSION_X: &str = "IMS:1000044";
pub(crate) const DIMENSION_Y: &str = "IMS:1000045";
pub(crate) const PIXEL_SIZE_X: &str = "IMS:1000046";
pub(crate) const PIXEL_SIZE_Y: &str = "IMS:1000047";
pub(crate) const POSITION_X: &str = "IMS:1000050";
pub(crate) const POSITION_Y: &str = "IMS:1000051";
pub(crate) const POSITION_Z: &str = "IMS:1000052";
pub(crate) const IBD_UUID: &str = "IMS:1000080";
pub(crate) const IBD_SHA1: &str = "IMS:1000091";
pub(crate) const EXTERNAL_OFFSET: &str = "IMS:1000102";
pub(crate) const EXTERNAL_ARRAY_LENGTH: &str = "IMS:1000103";
pub(crate) const EXTERNAL_ENCODED_LENGTH: &str = "IMS:1000104";
// mzMLb (PSI-MS): arrays in HDF5 datasets next to the XML
pub(crate) const EXTERNAL_HDF5_DATASET: &str = "MS:1002841";
pub(crate) const EXTERNAL_HDF5_OFFSET: &str = "MS:1002842";
pub(crate) const EXTERNAL_HDF5_LENGTH: &str = "MS:1002843";
// units
pub(crate) const UO_SECOND: &str = "UO:0000010";
pub(crate) const UO_MINUTE: &str = "UO:0000031";
pub(crate) const UO_MILLISECOND: &str = "UO:0000028";

/// Seconds per unit of a time value, from its unit accession or name (`None` = unknown unit).
pub(crate) fn seconds_per(p: &Param) -> Option<f64> {
    match (p.unit_accession.as_str(), p.unit_name.as_str()) {
        (UO_SECOND, _) | (_, "second") => Some(1.0),
        (UO_MINUTE, _) | (_, "minute") => Some(60.0),
        (UO_MILLISECOND, _) | (_, "millisecond") => Some(0.001),
        _ => None,
    }
}

/// Our short name for a dissociation method term (children of MS:1000044).
pub(crate) fn activation_name(accession: &str) -> Option<&'static str> {
    Some(match accession {
        "MS:1000133" | "MS:1000433" | "MS:1002472" => "CID",
        "MS:1000422" | "MS:1002481" => "HCD",
        "MS:1000598" => "ETD",
        "MS:1000250" => "ECD",
        "MS:1000262" => "IRMPD",
        "MS:1000435" => "PD",
        "MS:1003246" => "UVPD",
        "MS:1000599" => "PQD",
        "MS:1000136" => "SID",
        "MS:1003247" => "NETD",
        "MS:1003294" => "EAD",
        "MS:1000282" => "SORI",
        "MS:1000242" => "BIRD",
        "MS:1001880" => "ISCID",
        "MS:1002000" => "LIFT",
        "MS:1000134" => "PLASMA-DESORPTION",
        "MS:1000135" => "PSD",
        "MS:1002678" => "SUPPLEMENTAL-HCD",
        "MS:1002679" => "SUPPLEMENTAL-CID",
        _ => return None,
    })
}

/// Combine the dissociation terms of one `activation` element into one name.
pub(crate) fn activation(ps: &[Param]) -> Option<String> {
    let names: Vec<&str> = ps
        .iter()
        .filter_map(|p| activation_name(&p.accession))
        .collect();
    let has = |n: &str| names.contains(&n);
    if has("ETD") && has("SUPPLEMENTAL-HCD") {
        return Some("ETHCD".into());
    }
    if has("ETD") && has("SUPPLEMENTAL-CID") {
        return Some("ETCID".into());
    }
    let main: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !n.starts_with("SUPPLEMENTAL"))
        .collect();
    match main.as_slice() {
        [] => names.first().map(|n| (*n).to_string()),
        [one] => Some((*one).to_string()),
        many => Some(many.join("+")),
    }
}

/// `null-terminated ASCII string` (MS:1001479): a binary data type for arrays of text (OpenMS
/// writes its string meta-data arrays with it). Such an array holds no numbers.
pub(crate) const TEXT_ARRAY_TYPE: &str = "MS:1001479";

/// Whether a `binaryDataArray` holds text rather than numbers.
pub(crate) fn is_text_array(ps: &[Param]) -> bool {
    ps.iter().any(|p| p.accession == TEXT_ARRAY_TYPE)
}

/// Binary value type of a `binaryDataArray`.
pub(crate) fn value_type(ps: &[Param]) -> Option<ValueType> {
    ps.iter().find_map(|p| match p.accession.as_str() {
        "MS:1000521" => Some(ValueType::Float32),
        "MS:1000523" => Some(ValueType::Float64),
        "MS:1000519" => Some(ValueType::Int32),
        "MS:1000522" => Some(ValueType::Int64),
        _ => None,
    })
}

/// Compression of a `binaryDataArray` (absent = none, as writers that omit it intend).
pub(crate) fn compression(ps: &[Param]) -> Compression {
    for p in ps {
        let c = match p.accession.as_str() {
            "MS:1000576" => Compression::NoCompression,
            "MS:1000574" | "MS:1003088" => Compression::Zlib,
            "MS:1003780" => Compression::Zstd,
            "MS:1002312" => Compression::NumpressLinear(Outer::Plain),
            "MS:1002313" => Compression::NumpressPic(Outer::Plain),
            "MS:1002314" => Compression::NumpressSlof(Outer::Plain),
            "MS:1002746" => Compression::NumpressLinear(Outer::Zlib),
            "MS:1002747" => Compression::NumpressPic(Outer::Zlib),
            "MS:1002748" => Compression::NumpressSlof(Outer::Zlib),
            "MS:1003783" => Compression::NumpressLinear(Outer::Zstd),
            "MS:1003784" => Compression::NumpressPic(Outer::Zstd),
            "MS:1003785" => Compression::NumpressSlof(Outer::Zstd),
            "MS:1003089" => Compression::Unsupported(
                "truncation, delta prediction and zlib compression (MS:1003089)",
            ),
            "MS:1003090" => Compression::Unsupported(
                "truncation, linear prediction and zlib compression (MS:1003090)",
            ),
            "MS:1003781" => Compression::Unsupported("byte-shuffled zstd compression (MS:1003781)"),
            "MS:1003782" => {
                Compression::Unsupported("dictionary-encoded zstd compression (MS:1003782)")
            }
            "MS:1003826" => Compression::Unsupported("coordinate grid encoding (MS:1003826)"),
            _ => continue,
        };
        return c;
    }
    Compression::NoCompression
}

/// Which array a `binaryDataArray` holds: our key (`mz`, `intensity`, `time`, or the CV/user
/// name for anything else).
pub(crate) fn array_kind(ps: &[Param]) -> String {
    for p in ps {
        match p.accession.as_str() {
            MZ_ARRAY => return "mz".into(),
            INTENSITY_ARRAY => return "intensity".into(),
            TIME_ARRAY => return "time".into(),
            NON_STANDARD_ARRAY => {
                return if p.value.is_empty() {
                    p.name.clone()
                } else {
                    p.value.clone()
                };
            }
            _ => {}
        }
    }
    // other array terms end with "array" in their names
    ps.iter()
        .find(|p| p.name.ends_with(" array"))
        .map_or_else(|| "unknown array".into(), |p| p.name.clone())
}
