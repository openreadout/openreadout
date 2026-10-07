//! Assurance profiles (`docs/assurance.md`) of the NMR and JCAMP-DX readers (Bruker TopSpin,
//! JCAMP-DX, Agilent/Varian VnmrJ, JEOL Delta): the variant features of a data set and the
//! feature values the development corpus validates. The tables between the GENERATED markers
//! are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static BRUKER_NMR: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-nmr",
    observe: observe_bruker,
    validated: BRUKER_NMR_VALIDATED,
    confidence: BRUKER_NMR_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static JCAMP_DX: AssuranceProfile = AssuranceProfile {
    format_id: "jcamp-dx",
    observe: observe_jcamp,
    validated: JCAMP_DX_VALIDATED,
    confidence: JCAMP_DX_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static VARIAN_NMR: AssuranceProfile = AssuranceProfile {
    format_id: "varian-nmr",
    observe: observe_varian,
    validated: VARIAN_NMR_VALIDATED,
    confidence: VARIAN_NMR_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static JEOL_JDF: AssuranceProfile = AssuranceProfile {
    format_id: "jeol-jdf",
    observe: observe_jeol,
    validated: JEOL_JDF_VALIDATED,
    confidence: JEOL_JDF_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static MAGRITEK_SPINSOLVE: AssuranceProfile = AssuranceProfile {
    format_id: "magritek-spinsolve",
    observe: observe_spinsolve,
    validated: MAGRITEK_SPINSOLVE_VALIDATED,
    confidence: MAGRITEK_SPINSOLVE_CONFIDENCE,
    basis: Basis::PriorArt,
};

/// Features every NMR trace carries in the same vocabulary.
fn nmr_traces(o: &mut Observations, info: &FileInfo) {
    for t in &info.traces {
        if let Some(k) = a::extra_str(&t.extra, "kind") {
            o.feature(K::Record, k, &[Scope::Traces]);
        }
        if let Some(s) = a::extra_str(&t.extra, "sample_type") {
            o.feature(K::SampleLayout, s, &[Scope::Traces]);
        }
        if let Some(dims) = t
            .extra
            .get("indirect_dimensions")
            .and_then(serde_json::Value::as_array)
        {
            o.feature(
                K::Acquisition,
                format!("{}D", dims.len() + 1),
                &[Scope::Traces],
            );
            for d in dims {
                if let Some(e) = d.get("encoding").and_then(serde_json::Value::as_str) {
                    o.feature(K::Acquisition, format!("encoding {e}"), &[Scope::Traces]);
                }
            }
        }
        if t.extra.contains_key("non_uniform_sampling") {
            o.feature(K::Acquisition, "non-uniform sampling", &[Scope::Traces]);
        }
    }
}

fn observe_bruker(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    nmr_traces(&mut o, info);
    for t in &info.traces {
        if let Some(sw) = a::extra_str(&t.extra, "software") {
            // TopSpin and XWIN-NMR differ in parameter files and data layouts.
            let name = if sw.eq_ignore_ascii_case("topspin") {
                "TopSpin"
            } else {
                sw
            };
            o.feature(K::Writer, name, &[Scope::Metadata, Scope::Traces]);
            if let Some(v) =
                a::extra_str(&t.extra, "software_version").and_then(|v| a::version_prefix(v, 1))
            {
                o.context(K::WriterVersion, format!("{name} {v}"));
            }
        }
        if let Some(f) = a::extra_str(&t.extra, "file") {
            o.feature(K::Layout, f, &[Scope::Traces]);
        }
        if let Some(b) = a::extra_str(&t.extra, "byte_order") {
            o.feature(K::SampleLayout, b, &[Scope::Traces]);
        }
        if let Some(m) = a::extra_text(&t.extra, "acquisition_mode_code") {
            o.feature(K::Acquisition, format!("AQ_mod {m}"), &[Scope::Traces]);
        }
        if let Some(g) = a::extra_str(&t.extra, "group_delay_source") {
            o.context(K::Record, format!("group delay from {g}"));
        }
        if a::extra_str(&t.extra, "kind") == Some("time_domain") {
            o.calibration(
                "digital-filter group delay",
                CalibrationStatus::Available,
                &[Scope::Traces],
                "the FID is returned as stored; the group delay is in extra.group_delay_points and removed by `analyze nmr-peaks --from fid` / `trace --process`",
            );
        }
    }
    for n in &info.notes {
        if n.contains("the file's row count is used") || n.starts_with("no acqu2s") {
            o.assumed("traces[].sweep_count", n.clone());
        }
        if n.contains(" not readable: ") {
            o.undecoded("unreadable data file", &[], n.clone());
        }
        if n.contains("(truncated or a stopped acquisition)") {
            o.undecoded("rows missing from the data file", &[], n.clone());
        }
    }
    o
}

fn observe_jcamp(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(v) = a::extra_str(&t.extra, "jcamp_version") {
            o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
        }
        if let Some(d) = a::extra_str(&t.extra, "data_type") {
            o.feature(K::Acquisition, d.to_ascii_uppercase(), &[Scope::Traces]);
        }
        if let Some(k) = a::extra_str(&t.extra, "kind") {
            o.feature(K::Layout, k, &[Scope::Traces]);
        }
    }
    if info.traces.is_empty() {
        o.feature(K::Layout, "no data table", &[Scope::Metadata]);
    }
    o
}

fn observe_varian(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    match &info.format_version {
        Some(v) => o.feature(K::FormatVersion, v, &[Scope::Metadata]),
        None => o.feature(K::FormatVersion, "no procpar", &[Scope::Metadata]),
    }
    nmr_traces(&mut o, info);
    for t in &info.traces {
        if let Some(i) = a::extra_str(&t.extra, "instrument") {
            o.context(K::Instrument, i);
        }
    }
    if !info.traces.is_empty() {
        o.calibration(
            "block scale factors and drift correction",
            CalibrationStatus::Available,
            &[Scope::Traces],
            "values are returned as stored (as nmrglue reads them); the block scale factors and drift corrections are reported, not applied",
        );
    }
    if let Some(n) = a::note_with(info, "arrayed and multidimensional data are not reordered") {
        o.assumed("traces[].sweeps order", n.to_string());
    }
    o
}

fn observe_jeol(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        // "JDF 1.2, Delta 5.3.1 [Windows]": the container version is structural, Delta's descriptive.
        let (jdf, delta) = v.split_once(", ").unwrap_or((v, ""));
        o.feature(K::FormatVersion, jdf, &[Scope::Metadata, Scope::Traces]);
        if let Some(d) = a::version_prefix(delta, 1) {
            o.context(K::WriterVersion, format!("Delta {d}"));
        }
    }
    nmr_traces(&mut o, info);
    for t in &info.traces {
        if let Some(f) = a::extra_str(&t.extra, "data_format") {
            o.feature(K::Layout, f, &[Scope::Traces]);
        }
        for ax in a::extra_values_in(&t.extra, "axis_types") {
            o.feature(K::SampleLayout, format!("axis {ax}"), &[Scope::Traces]);
        }
        if let Some(i) = a::extra_str(&t.extra, "instrument") {
            o.context(K::Instrument, i);
        }
        if t.extra.get("sample_id_truncated") == Some(&serde_json::Value::Bool(true)) {
            o.assumed(
                "traces[].extra.sample_id",
                "the sample_id parameter fills its 16 bytes and the file's context section holds no longer value: the id may be cut",
            );
        }
    }
    for n in &info.notes {
        if n.starts_with("data not decoded: unsupported") {
            o.undecoded("unsupported data layout", &[], n.clone());
        }
    }
    o
}

fn observe_spinsolve(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    nmr_traces(&mut o, info);
    for t in &info.traces {
        if let Some(code) = t.extra.get("data_type").and_then(serde_json::Value::as_u64) {
            o.feature(
                K::SampleLayout,
                format!("data type {code}"),
                &[Scope::Traces],
            );
        }
        if let Some(f) = a::extra_str(&t.extra, "file") {
            // data.1d / data.2d / spectrum.1d …: the extension says the dimensionality
            let ext = f.rsplit('.').next().unwrap_or(f);
            o.feature(K::Layout, ext, &[Scope::Traces]);
        }
        if let Some(v) =
            a::extra_str(&t.extra, "software_version").and_then(|v| a::version_prefix(v, 2))
        {
            o.context(K::WriterVersion, format!("Spinsolve {v}"));
        }
        if let Some(i) = a::extra_str(&t.extra, "instrument") {
            o.context(K::Instrument, i);
        }
    }
    for n in &info.notes {
        if n.contains("listed, not decoded") || n.contains("are not decoded") {
            o.undecoded("data layout", &[], n.clone());
        }
    }
    o
}

// BEGIN GENERATED bruker-nmr (cargo xtask assurance-audit --write; do not edit)
const BRUKER_NMR_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BRUKER_NMR_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "2D", 13, 11, 14),
    a::row(K::Acquisition, "AQ_mod 1", 4, 4, 4),
    a::row(K::Acquisition, "AQ_mod 3", 57, 33, 58),
    a::row(K::Acquisition, "encoding Echo-Antiecho", 4, 4, 5),
    a::row(K::Acquisition, "encoding QF", 2, 2, 2),
    a::row(K::Acquisition, "encoding States-TPPI", 4, 4, 4),
    a::row(K::Acquisition, "encoding undefined", 3, 2, 3),
    a::row(K::Acquisition, "non-uniform sampling", 3, 3, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 11, 6, 62),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 18),
    a::row(K::Layout, "fid", 48, 27, 48),
    a::row(K::Layout, "ser", 13, 11, 14),
    a::row(K::Record, "group delay from DSPFVS/DECIM table", 7, 5, 7),
    a::row(K::Record, "group delay from GRPDLY", 54, 31, 55),
    a::row(K::Record, "processed_spectrum", 54, 32, 55),
    a::row(K::Record, "time_domain", 61, 35, 62),
    a::row(K::SampleLayout, "big-endian", 7, 5, 7),
    a::row(K::SampleLayout, "float64", 10, 6, 10),
    a::row(K::SampleLayout, "int32", 61, 35, 62),
    a::row(K::SampleLayout, "little-endian", 58, 34, 59),
    a::row(K::Writer, "TopSpin", 58, 34, 59),
    a::row(K::Writer, "XWIN-NMR", 3, 1, 3),
    a::row(K::WriterVersion, "TopSpin 1", 2, 2, 2),
    a::row(K::WriterVersion, "TopSpin 2", 9, 3, 9),
    a::row(K::WriterVersion, "TopSpin 3", 33, 21, 34),
    a::row(K::WriterVersion, "TopSpin 4", 14, 9, 14),
    a::row(K::WriterVersion, "XWIN-NMR 3", 3, 1, 3),
];
// END GENERATED bruker-nmr

// BEGIN GENERATED jcamp-dx (cargo xtask assurance-audit --write; do not edit)
const JCAMP_DX_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const JCAMP_DX_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "CONTINUOUS MASS SPECTRUM", 0, 0, 1),
    a::row(K::Acquisition, "INFRARED SPECTRUM", 6, 3, 6),
    a::row(K::Acquisition, "MASS SPECTRUM", 2, 1, 4),
    a::row(K::Acquisition, "ND NMR SPECTRUM", 0, 0, 4),
    a::row(K::Acquisition, "NMR FID", 1, 1, 1),
    a::row(K::Acquisition, "NMR PEAK TABLE", 1, 1, 1),
    a::row(K::Acquisition, "NMR SPECTRUM", 13, 6, 14),
    a::row(K::Acquisition, "NMRPEAKTABLE", 1, 1, 1),
    a::row(K::Acquisition, "NMRSPECTRUM", 1, 1, 1),
    a::row(K::Acquisition, "UV-VISIBLE SPECTRUM", 1, 1, 1),
    a::row(K::Acquisition, "UV/VIS SPECTRUM", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 3, 2, 8),
    a::row(K::Field, "experiment.instrument.model", 13, 2, 18),
    a::row(K::FormatVersion, "4.24", 9, 3, 9),
    a::row(K::FormatVersion, "5", 1, 1, 1),
    a::row(K::FormatVersion, "5.0", 6, 1, 6),
    a::row(K::FormatVersion, "5.00", 3, 1, 4),
    a::row(K::FormatVersion, "5.01", 1, 1, 1),
    a::row(K::FormatVersion, "6.0", 12, 9, 12),
    a::row(K::Layout, "ntuples", 4, 3, 9),
    a::row(K::Layout, "peak_table", 4, 3, 6),
    a::row(K::Layout, "xydata", 19, 7, 20),
];
// END GENERATED jcamp-dx

// BEGIN GENERATED varian-nmr (cargo xtask assurance-audit --write; do not edit)
const VARIAN_NMR_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const VARIAN_NMR_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "2D", 4, 3, 4),
    a::row(K::Acquisition, "3D", 1, 1, 1),
    a::row(K::Acquisition, "4D", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 11, 6, 12),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 12),
    a::row(K::FormatVersion, "VnmrJ VERSION 4.0 REVISION A", 2, 2, 2),
    a::row(K::FormatVersion, "VnmrJ VERSION 4.2 REVISION A", 5, 4, 5),
    a::row(K::FormatVersion, "no procpar", 1, 1, 1),
    a::row(K::FormatVersion, "procpar version 5.1", 5, 2, 5),
    a::row(K::Instrument, "inova", 2, 1, 2),
    a::row(K::Instrument, "vnmrs", 10, 6, 10),
    a::row(K::Record, "time_domain", 13, 7, 13),
    a::row(K::SampleLayout, "float32", 12, 7, 12),
    a::row(K::SampleLayout, "int32", 1, 1, 1),
];
// END GENERATED varian-nmr

// BEGIN GENERATED jeol-jdf (cargo xtask assurance-audit --write; do not edit)
const JEOL_JDF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const JEOL_JDF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "2D", 5, 3, 5),
    a::row(K::Acquisition, "encoding complex", 3, 2, 3),
    a::row(K::Acquisition, "encoding real_complex", 2, 2, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 12, 7, 25),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 19),
    a::row(K::FormatVersion, "JDF 1.1", 3, 2, 3),
    a::row(K::FormatVersion, "JDF 1.2", 22, 16, 22),
    a::row(K::Instrument, "JNM-ECX400", 2, 1, 2),
    a::row(K::Instrument, "JNM-ECX500", 1, 1, 1),
    a::row(K::Instrument, "JNM-ECZ400S/L1", 7, 3, 7),
    a::row(K::Instrument, "JNM-ECZ500R/M1", 3, 2, 3),
    a::row(K::Instrument, "JNM-ECZ500R/S1", 2, 2, 2),
    a::row(K::Instrument, "JNM-ECZ600R/S1", 2, 1, 2),
    a::row(K::Instrument, "NM-70020R4S1", 1, 1, 1),
    a::row(K::Instrument, "NM-70050G4", 1, 1, 1),
    a::row(K::Layout, "one_d", 20, 17, 20),
    a::row(K::Layout, "two_d", 5, 3, 5),
    a::row(K::Record, "processed_spectrum", 5, 4, 5),
    a::row(K::Record, "time_domain", 20, 14, 20),
    a::row(K::SampleLayout, "axis complex", 22, 16, 22),
    a::row(K::SampleLayout, "axis real", 1, 1, 1),
    a::row(K::SampleLayout, "axis real_complex", 2, 2, 2),
    a::row(K::SampleLayout, "float64", 25, 18, 25),
    a::row(K::WriterVersion, "Delta 4", 3, 2, 3),
    a::row(K::WriterVersion, "Delta 5", 10, 6, 10),
    a::row(K::WriterVersion, "Delta 6", 9, 7, 9),
];
// END GENERATED jeol-jdf

// BEGIN GENERATED magritek-spinsolve (cargo xtask assurance-audit --write; do not edit)
const MAGRITEK_SPINSOLVE_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const MAGRITEK_SPINSOLVE_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 26),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 24),
    a::row(K::Instrument, "C43", 10, 1, 10),
    a::row(K::Instrument, "C60Ultra", 10, 1, 10),
    a::row(K::Instrument, "C80Ultra", 3, 1, 3),
    a::row(K::Instrument, "P60Grad", 1, 1, 1),
    a::row(K::Layout, "1d", 14, 2, 14),
    a::row(K::Layout, "2d", 13, 2, 13),
    a::row(K::Record, "spectrum", 1, 1, 1),
    a::row(K::Record, "time_domain", 24, 2, 24),
    a::row(K::SampleLayout, "data type 501", 24, 2, 24),
    a::row(K::SampleLayout, "data type 503", 1, 1, 1),
    a::row(K::SampleLayout, "data type 504", 3, 1, 3),
    a::row(K::WriterVersion, "Spinsolve 1.41", 3, 1, 3),
    a::row(K::WriterVersion, "Spinsolve 2.0", 1, 1, 1),
    a::row(K::WriterVersion, "Spinsolve 2.1", 10, 1, 10),
    a::row(K::WriterVersion, "Spinsolve 2.2", 10, 1, 10),
];
// END GENERATED magritek-spinsolve
