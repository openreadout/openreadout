//! Assurance profiles (`docs/assurance.md`) of the open mass-spectrometry formats (mzML, imzML,
//! mzXML, mzMLb): the variant features of a file and the feature values the development
//! corpus validates. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static MZML: AssuranceProfile = AssuranceProfile {
    format_id: "mzml",
    observe,
    validated: MZML_VALIDATED,
    confidence: MZML_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static IMZML: AssuranceProfile = AssuranceProfile {
    format_id: "imzml",
    observe,
    validated: IMZML_VALIDATED,
    confidence: IMZML_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static MZXML: AssuranceProfile = AssuranceProfile {
    format_id: "mzxml",
    observe,
    validated: MZXML_VALIDATED,
    confidence: MZXML_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static MZMLB: AssuranceProfile = AssuranceProfile {
    format_id: "mzmlb",
    observe,
    validated: MZMLB_VALIDATED,
    confidence: MZMLB_CONFIDENCE,
    basis: Basis::OpenSpec,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Spectra, Scope::Traces],
        );
    }
    for s in &info.spectra {
        // The converter (msconvert, psims, a vendor exporter) decides which optional terms and
        // encodings the document uses.
        if let Some(sw) = s
            .extra
            .get("software")
            .and_then(serde_json::Value::as_array)
        {
            for w in sw {
                if let Some(n) = w.get("name").and_then(serde_json::Value::as_str) {
                    o.context(K::Writer, n);
                }
            }
        }
        for c in a::extra_values_in(&s.extra, "file_content") {
            o.feature(K::Acquisition, c, &[Scope::Spectra, Scope::Traces]);
        }
        if let Some(i) = &s.instrument
            && let Some(m) = &i.model
        {
            o.context(K::Instrument, m);
        }
        if let Some(v) = s
            .extra
            .get("container")
            .and_then(|c| c.get("mzmlb_version"))
            .and_then(serde_json::Value::as_str)
        {
            o.feature(K::FormatVersion, v, &[Scope::Spectra, Scope::Traces]);
        }
        if s.extra.contains_key("imaging") {
            o.feature(K::Layout, "imaging (.ibd)", &[Scope::Spectra]);
        }
        if s.extra.contains_key("compression") {
            o.feature(K::Codec, "gzip container", &[Scope::Spectra, Scope::Traces]);
        }
    }
    for t in &info.traces {
        if let Some(c) = a::extra_str(&t.extra, "chromatogram_type") {
            o.feature(K::Record, c, &[Scope::Traces]);
        }
    }
    for n in &info.notes {
        if n.contains("were located by scanning the file") {
            o.feature(K::Layout, "index rebuilt by scanning", &[Scope::Spectra]);
        }
        if n.starts_with("chromatograms could not be read") {
            o.undecoded("chromatograms", &[], n.clone());
        }
    }
    o
}

// BEGIN GENERATED mzml (cargo xtask assurance-audit --write; do not edit)
const MZML_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const MZML_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "MS1 spectrum", 42, 30, 77),
    a::row(K::Acquisition, "MSn spectrum", 37, 26, 75),
    a::row(K::Acquisition, "SRM spectrum", 0, 0, 1),
    a::row(K::Acquisition, "centroid spectrum", 4, 4, 9),
    a::row(K::Acquisition, "constant neutral loss spectrum", 0, 0, 2),
    a::row(K::Acquisition, "electromagnetic radiation spectrum", 0, 0, 1),
    a::row(K::Acquisition, "ion current chromatogram", 4, 4, 4),
    a::row(K::Acquisition, "mass spectrum", 1, 1, 1),
    a::row(K::Acquisition, "precursor ion spectrum", 0, 0, 1),
    a::row(K::Acquisition, "profile spectrum", 1, 1, 10),
    a::row(K::Acquisition, "selected ion monitoring chromatogram", 0, 0, 3),
    a::row(K::Acquisition, "selected reaction monitoring chromatogram", 5, 5, 10),
    a::row(K::Acquisition, "total ion current chromatogram", 5, 5, 19),
    a::row(K::Codec, "gzip container", 3, 1, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 49, 39, 110),
    a::row(K::FormatVersion, "1.1.0", 53, 40, 117),
    a::row(K::Instrument, "4000 QTRAP", 0, 0, 1),
    a::row(K::Instrument, "AB SCIEX instrument model", 0, 0, 1),
    a::row(K::Instrument, "Agilent instrument model", 5, 5, 19),
    a::row(K::Instrument, "Applied Biosystems instrument model", 0, 0, 1),
    a::row(K::Instrument, "Bruker Daltonics timsTOF series", 2, 1, 9),
    a::row(K::Instrument, "Exactive", 1, 1, 1),
    a::row(K::Instrument, "LCQ Deca", 1, 1, 1),
    a::row(K::Instrument, "LTQ FT", 7, 2, 7),
    a::row(K::Instrument, "LTQ Orbitrap", 1, 1, 1),
    a::row(K::Instrument, "LTQ Orbitrap Discovery", 6, 2, 6),
    a::row(K::Instrument, "LTQ Orbitrap Elite", 1, 1, 1),
    a::row(K::Instrument, "LTQ Orbitrap Velos", 1, 1, 1),
    a::row(K::Instrument, "LTQ Orbitrap XL", 1, 1, 1),
    a::row(K::Instrument, "LTQ Velos", 0, 0, 2),
    a::row(K::Instrument, "LTQ XL", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Ascend", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Astral", 2, 2, 2),
    a::row(K::Instrument, "Orbitrap Eclipse", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Exploris 240", 2, 2, 2),
    a::row(K::Instrument, "Orbitrap Exploris 480", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Fusion", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Fusion Lumos", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap ID-X", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap IQ-X", 1, 1, 1),
    a::row(K::Instrument, "Q Exactive", 4, 3, 4),
    a::row(K::Instrument, "Q Exactive HF", 1, 1, 1),
    a::row(K::Instrument, "Q Exactive HF-X", 2, 2, 2),
    a::row(K::Instrument, "QTRAP 5500", 0, 0, 1),
    a::row(K::Instrument, "QTRAP 6500", 1, 1, 5),
    a::row(K::Instrument, "Stellar", 0, 0, 2),
    a::row(K::Instrument, "TSQ Vantage", 1, 1, 1),
    a::row(K::Instrument, "Thermo Electron instrument model", 1, 1, 11),
    a::row(K::Instrument, "TripleTOF 5600", 1, 1, 1),
    a::row(K::Instrument, "TripleTOF 6600", 1, 1, 1),
    a::row(K::Instrument, "Waters instrument model", 2, 2, 23),
    a::row(K::Instrument, "instrument model", 2, 2, 2),
    a::row(K::Layout, "index rebuilt by scanning", 3, 2, 3),
    a::row(K::Record, "absorption chromatogram", 0, 0, 2),
    a::row(K::Record, "basepeak chromatogram", 4, 2, 24),
    a::row(K::Record, "chromatogram", 0, 0, 2),
    a::row(K::Record, "electromagnetic radiation chromatogram", 0, 0, 1),
    a::row(K::Record, "emission chromatogram", 0, 0, 1),
    a::row(K::Record, "flow rate chromatogram", 0, 0, 8),
    a::row(K::Record, "pressure chromatogram", 0, 0, 14),
    a::row(K::Record, "selected ion current chromatogram", 1, 1, 1),
    a::row(K::Record, "selected ion monitoring chromatogram", 0, 0, 3),
    a::row(K::Record, "selected reaction monitoring chromatogram", 5, 5, 11),
    a::row(K::Record, "temperature chromatogram", 0, 0, 3),
    a::row(K::Record, "total ion current chromatogram", 16, 7, 112),
    a::row(K::Writer, "Analyst", 3, 3, 11),
    a::row(K::Writer, "Bioworks", 1, 1, 1),
    a::row(K::Writer, "Bruker software", 2, 1, 9),
    a::row(K::Writer, "Compass", 0, 0, 4),
    a::row(K::Writer, "CompassXtract", 1, 1, 1),
    a::row(K::Writer, "FileConverter", 1, 1, 1),
    a::row(K::Writer, "MassHunter Data Acquisition", 5, 5, 19),
    a::row(K::Writer, "MassLynx", 2, 2, 23),
    a::row(K::Writer, "ProteinPilot Software", 0, 0, 1),
    a::row(K::Writer, "ProteoWizard", 4, 3, 4),
    a::row(K::Writer, "ProteoWizard software", 43, 32, 106),
    a::row(K::Writer, "ThermoRawFileParser", 2, 2, 2),
    a::row(K::Writer, "Xcalibur", 34, 25, 48),
    a::row(K::Writer, "custom unreleased software tool", 3, 3, 3),
    a::row(K::Writer, "micrOTOFcontrol", 2, 1, 5),
    a::row(K::Writer, "ms_deisotope", 1, 1, 1),
    a::row(K::Writer, "python-psims", 2, 0, 2),
];
// END GENERATED mzml

// BEGIN GENERATED imzml (cargo xtask assurance-audit --write; do not edit)
const IMZML_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const IMZML_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "MS1 spectrum", 2, 1, 2),
    a::row(K::Acquisition, "continuous", 1, 1, 1),
    a::row(K::Acquisition, "ibd SHA-1", 2, 1, 2),
    a::row(K::Acquisition, "processed", 1, 1, 1),
    a::row(K::Acquisition, "profile spectrum", 2, 1, 2),
    a::row(K::Acquisition, "universally unique identifier", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 2),
    a::row(K::FormatVersion, "1.1", 2, 1, 2),
    a::row(K::Instrument, "LTQ FT Ultra", 2, 1, 2),
    a::row(K::Layout, "imaging (.ibd)", 2, 1, 2),
    a::row(K::Writer, "Xcalibur", 2, 1, 2),
    a::row(K::Writer, "custom unreleased software tool", 2, 1, 2),
];
// END GENERATED imzml

// BEGIN GENERATED mzxml (cargo xtask assurance-audit --write; do not edit)
const MZXML_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const MZXML_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "gzip container", 1, 0, 1),
    a::row(K::FormatVersion, "2.0", 2, 1, 2),
    a::row(K::FormatVersion, "2.1", 0, 0, 1),
    a::row(K::FormatVersion, "3.0", 1, 1, 1),
    a::row(K::FormatVersion, "3.1", 1, 1, 1),
    a::row(K::FormatVersion, "3.2", 3, 3, 4),
    a::row(K::Instrument, "API 2000", 0, 0, 1),
    a::row(K::Instrument, "Applied Biosystems instrument model", 1, 1, 1),
    a::row(K::Instrument, "LCQ Deca", 2, 1, 2),
    a::row(K::Instrument, "LTQ", 1, 1, 1),
    a::row(K::Instrument, "LTQ FT", 1, 1, 1),
    a::row(K::Instrument, "LTQ Orbitrap Velos", 1, 1, 1),
    a::row(K::Instrument, "Orbitrap Exploris 120", 1, 1, 1),
    a::row(K::Layout, "index rebuilt by scanning", 2, 1, 2),
];
// END GENERATED mzxml

// BEGIN GENERATED mzmlb (cargo xtask assurance-audit --write; do not edit)
const MZMLB_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const MZMLB_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "MS1 spectrum", 4, 1, 4),
    a::row(K::Acquisition, "MSn spectrum", 4, 1, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 1, 1, 1),
    a::row(K::FormatVersion, "1.1.0", 4, 1, 4),
    a::row(K::FormatVersion, "mzMLb 1.0", 4, 1, 4),
    a::row(K::Instrument, "LTQ FT", 4, 1, 4),
    a::row(K::Record, "total ion current chromatogram", 4, 1, 4),
    a::row(K::Writer, "ProteoWizard software", 1, 1, 1),
    a::row(K::Writer, "Xcalibur", 1, 1, 1),
    a::row(K::Writer, "python-psims", 3, 0, 3),
];
// END GENERATED mzmlb
