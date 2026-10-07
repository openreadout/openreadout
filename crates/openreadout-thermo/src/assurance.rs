//! Assurance profile (`docs/assurance.md`): the variant features of a Thermo `.raw` file (file
//! version, instrument generation, analyzers, scan types, stored spectra, detector
//! controllers) and the feature values the development corpus validates. The table between
//! the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static THERMO_RAW: AssuranceProfile = AssuranceProfile {
    format_id: "thermo-raw",
    observe,
    validated: THERMO_RAW_VALIDATED,
    confidence: THERMO_RAW_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// The scan type of a filter string: `FTMS - p ESI Full ms [50.00-1000.00]` → `Full ms`,
/// `FTMS {1,1}  + p ESI Full lock ms [..]` → `Full lock ms`, `+ c NSI SRM ms2 ...` → `SRM ms2`.
pub(crate) fn filter_scan_type(filter: &str) -> Option<String> {
    let head = filter.split('[').next()?.trim();
    let words: Vec<&str> = head.split_whitespace().collect();
    let start = words.iter().position(|w| {
        matches!(
            *w,
            "Full" | "SIM" | "SRM" | "CRM" | "Z" | "Q1MS" | "Q3MS" | "MSX" | "msx"
        )
    })?;
    let end = words[start..]
        .iter()
        .position(|w| w.starts_with("ms"))
        .map_or(words.len(), |i| start + i + 1);
    Some(words[start..end].join(" "))
}

/// The instrument generation a model belongs to: models of one generation share the scan
/// packet and trailer layouts (`docs/formats/thermo-raw.md`: the Exploris annotation block, the
/// v64 Exactive transition, the TSQ window records). An unknown model is its own generation.
pub(crate) fn instrument_generation(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    let g = if m.contains("exploris") {
        "generation Orbitrap Exploris"
    } else if m.contains("astral") {
        "generation Orbitrap Astral"
    } else if ["fusion", "eclipse", "id-x", "iq-x", "ascend", "tribrid"]
        .iter()
        .any(|k| m.contains(k))
    {
        "generation Orbitrap Tribrid"
    } else if m.contains("q exactive") {
        "generation Q Exactive"
    } else if m.contains("exactive") {
        "generation Exactive"
    } else if m.contains("ltq orbitrap") || m.contains("orbitrap elite") {
        "generation LTQ Orbitrap"
    } else if m.contains("ltq ft") {
        // An ion-cyclotron hybrid: its FT profiles use the four-coefficient m/z conversion.
        "generation LTQ FT"
    } else if m.contains("ltq") || m.contains("velos pro") {
        "generation LTQ ion trap"
    } else if m.contains("tsq") {
        "generation TSQ"
    } else {
        return model.to_string();
    };
    g.to_string()
}

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Spectra, Scope::Traces];
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &all);
    }
    for s in &info.spectra {
        if let Some(i) = &s.instrument {
            // Scan packets and trailers change with the instrument generation (Exploris vs
            // Q Exactive vs LTQ): the generation is structural for spectra; the exact model
            // (`Q Exactive HF Orbitrap`, `Orbitrap Exploris Slot #…`) only describes the file.
            if let Some(m) = &i.model {
                o.feature(K::Instrument, instrument_generation(m), &[Scope::Spectra]);
                o.context(K::Instrument, m);
            }
            if let Some(v) = i
                .software_version
                .as_deref()
                .and_then(|v| a::version_prefix(v, 2))
            {
                o.context(K::WriterVersion, format!("Xcalibur {v}"));
            }
        }
        for an in a::extra_values_in(&s.extra, "analyzers") {
            o.feature(K::Acquisition, format!("analyzer {an}"), &[Scope::Spectra]);
        }
        if s.extra
            .get("profile_scans")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            o.feature(K::Acquisition, "profile spectra", &[Scope::Spectra]);
        }
        if s.extra
            .get("centroid_scans")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            o.feature(K::Acquisition, "centroid spectra", &[Scope::Spectra]);
        }
        if let Some(levels) = s
            .extra
            .get("ms_level_counts")
            .and_then(serde_json::Value::as_object)
        {
            for l in levels.keys() {
                o.feature(K::Acquisition, format!("ms{l}"), &[Scope::Spectra]);
            }
        }
        if let Some(f) = s
            .extra
            .get("ms1_scan_filters")
            .and_then(serde_json::Value::as_object)
        {
            for k in f.keys() {
                if let Some(t) = filter_scan_type(k) {
                    o.feature(K::Acquisition, t, &[Scope::Spectra]);
                }
            }
        }
        // FAIMS and in-source CID change what an event's tail items mean (and the filter text).
        for opt in a::extra_values_in(&s.extra, "scan_options") {
            o.feature(K::Acquisition, opt, &[Scope::Spectra]);
        }
        let pol = a::extra_values_in(&s.extra, "polarities");
        if pol.len() > 1 {
            o.feature(K::Acquisition, "polarity switching", &[Scope::Spectra]);
        }
        for src in a::extra_values_in(&s.extra, "ion_sources") {
            o.context(K::Acquisition, format!("ion source {src}"));
        }
    }
    for (i, t) in info.traces.iter().enumerate() {
        if let Some(d) = a::extra_str(&t.extra, "detector") {
            o.feature(K::Record, format!("controller {d}"), &[Scope::Traces]);
        }
        if a::extra_str(&t.extra, "unit_source") == Some("inferred") {
            o.assumed(
                format!("traces[{i}].channels[].unit"),
                "the file records no unit for this detector trace; it was inferred from the device",
            );
        }
    }
    o
}

// BEGIN GENERATED thermo-raw (cargo xtask assurance-audit --write; do not edit)
const THERMO_RAW_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const THERMO_RAW_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "FAIMS", 3, 3, 3),
    a::row(K::Acquisition, "Full lock ms", 1, 1, 1),
    a::row(K::Acquisition, "Full ms", 33, 29, 34),
    a::row(K::Acquisition, "SIM ms", 1, 1, 1),
    a::row(K::Acquisition, "Z ms", 1, 1, 1),
    a::row(K::Acquisition, "analyzer ASTMS", 2, 2, 2),
    a::row(K::Acquisition, "analyzer FTMS", 35, 27, 36),
    a::row(K::Acquisition, "analyzer ITMS", 10, 8, 10),
    a::row(K::Acquisition, "analyzer code 6", 4, 4, 4),
    a::row(K::Acquisition, "centroid spectra", 22, 20, 23),
    a::row(K::Acquisition, "in-source CID", 1, 1, 2),
    a::row(K::Acquisition, "ion source EI", 2, 2, 2),
    a::row(K::Acquisition, "ion source ESI", 21, 17, 22),
    a::row(K::Acquisition, "ion source MALDI", 2, 1, 2),
    a::row(K::Acquisition, "ion source NSI", 19, 13, 19),
    a::row(K::Acquisition, "ms1", 33, 29, 34),
    a::row(K::Acquisition, "ms2", 33, 26, 34),
    a::row(K::Acquisition, "ms3", 3, 2, 3),
    a::row(K::Acquisition, "ms4", 1, 1, 1),
    a::row(K::Acquisition, "ms5", 1, 1, 1),
    a::row(K::Acquisition, "polarity switching", 4, 4, 4),
    a::row(K::Acquisition, "profile spectra", 34, 25, 34),
    a::row(K::Field, "experiment.acquisition.started_at", 37, 27, 45),
    a::row(K::Field, "experiment.instrument.model", 22, 17, 40),
    a::row(K::FormatVersion, "57", 1, 1, 1),
    a::row(K::FormatVersion, "61", 1, 1, 1),
    a::row(K::FormatVersion, "62", 1, 1, 1),
    a::row(K::FormatVersion, "63", 7, 3, 7),
    a::row(K::FormatVersion, "64", 7, 7, 7),
    a::row(K::FormatVersion, "66", 27, 21, 28),
    a::row(K::Instrument, "ISQ", 1, 1, 1),
    a::row(K::Instrument, "LTQ", 1, 1, 1),
    a::row(K::Instrument, "LTQ FT Ultra", 1, 1, 1),
    a::row(K::Instrument, "LTQ Orbitrap", 1, 1, 1),
    a::row(K::Instrument, "LTQ Orbitrap Discovery", 6, 2, 6),
    a::row(K::Instrument, "LTQ Orbitrap Velos", 2, 2, 2),
    a::row(K::Instrument, "LTQ Orbitrap XL", 1, 1, 1),
    a::row(K::Instrument, "LTQ Velos", 1, 1, 1),
    a::row(K::Instrument, "LTQ XL", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Ascend", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Astral", 2, 2, 2),
    a::row(K::Instrument, "Orbitrap Eclipse", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Elite", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Exploris 120", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Exploris 240", 2, 2, 2),
    a::row(K::Instrument, "Orbitrap Exploris 480", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Fusion Lumos", 2, 2, 2),
    a::row(K::Instrument, "Orbitrap ID-X", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap IQ-X", 1, 1, 1),
    a::row(K::Instrument, "Q Exactive HF Orbitrap", 3, 3, 3),
    a::row(K::Instrument, "Q Exactive HF-X Orbitrap", 2, 2, 2),
    a::row(K::Instrument, "Q Exactive Plus Orbitrap", 1, 1, 2),
    a::row(K::Instrument, "Stellar", 1, 1, 1),
    a::row(K::Instrument, "TSQ 9610", 1, 1, 1),
    a::row(K::Instrument, "TSQ Altis Plus", 1, 1, 1),
    a::row(K::Instrument, "TSQ Vantage Standard", 1, 1, 1),
    a::row(K::Instrument, "Thermo Exactive Orbitrap", 1, 1, 1),
    a::row(K::Instrument, "generation Exactive", 1, 1, 1),
    a::row(K::Instrument, "generation LTQ FT", 1, 1, 1),
    a::row(K::Instrument, "generation LTQ Orbitrap", 11, 7, 11),
    a::row(K::Instrument, "generation LTQ ion trap", 3, 3, 3),
    a::row(K::Instrument, "generation Orbitrap Astral", 2, 2, 2),
    a::row(K::Instrument, "generation Orbitrap Exploris", 4, 4, 4),
    a::row(K::Instrument, "generation Orbitrap Tribrid", 6, 6, 6),
    a::row(K::Instrument, "generation Q Exactive", 6, 5, 7),
    a::row(K::Instrument, "generation TSQ", 3, 3, 3),
    a::row(K::Record, "controller analog", 0, 0, 15),
    a::row(K::Record, "controller channel", 1, 1, 2),
    a::row(K::Record, "controller pda", 1, 1, 2),
    a::row(K::WriterVersion, "Xcalibur 1.0", 2, 2, 2),
    a::row(K::WriterVersion, "Xcalibur 1.1", 3, 3, 3),
    a::row(K::WriterVersion, "Xcalibur 2.0", 2, 2, 2),
    a::row(K::WriterVersion, "Xcalibur 2.11", 0, 0, 1),
    a::row(K::WriterVersion, "Xcalibur 2.13", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 2.2", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 2.3", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 2.4", 7, 3, 7),
    a::row(K::WriterVersion, "Xcalibur 2.5", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 2.6", 4, 4, 4),
    a::row(K::WriterVersion, "Xcalibur 2.7", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 2.8", 2, 2, 2),
    a::row(K::WriterVersion, "Xcalibur 2.9", 2, 2, 2),
    a::row(K::WriterVersion, "Xcalibur 3.0", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 3.1", 2, 2, 2),
    a::row(K::WriterVersion, "Xcalibur 3.3", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 3.4", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 3.5", 2, 2, 2),
    a::row(K::WriterVersion, "Xcalibur 4.2", 3, 3, 3),
    a::row(K::WriterVersion, "Xcalibur 4.3", 1, 1, 1),
    a::row(K::WriterVersion, "Xcalibur 5.1", 1, 1, 1),
];
// END GENERATED thermo-raw
