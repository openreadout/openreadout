//! Assurance profiles (`docs/assurance.md`) of the vibrational-spectroscopy readers (Bruker
//! OPUS, Thermo OMNIC, Renishaw WiRE, PerkinElmer `.sp`): the variant features of a file and
//! the feature values the development corpus validates. The tables between the GENERATED
//! markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static BRUKER_OPUS: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-opus",
    observe: observe_opus,
    validated: BRUKER_OPUS_VALIDATED,
    confidence: BRUKER_OPUS_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static THERMO_OMNIC: AssuranceProfile = AssuranceProfile {
    format_id: "thermo-omnic",
    observe: observe_omnic,
    validated: THERMO_OMNIC_VALIDATED,
    confidence: THERMO_OMNIC_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static RENISHAW_WDF: AssuranceProfile = AssuranceProfile {
    format_id: "renishaw-wdf",
    observe: observe_wdf,
    validated: RENISHAW_WDF_VALIDATED,
    confidence: RENISHAW_WDF_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static PERKINELMER_SP: AssuranceProfile = AssuranceProfile {
    format_id: "perkinelmer-sp",
    observe: observe_pesp,
    validated: PERKINELMER_SP_VALIDATED,
    confidence: PERKINELMER_SP_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static GALACTIC_SPC: AssuranceProfile = AssuranceProfile {
    format_id: "galactic-spc",
    observe: observe_spc,
    validated: GALACTIC_SPC_VALIDATED,
    confidence: GALACTIC_SPC_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static JASCO_JWS: AssuranceProfile = AssuranceProfile {
    format_id: "jasco-jws",
    observe: observe_jws,
    validated: JASCO_JWS_VALIDATED,
    confidence: JASCO_JWS_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static WITEC_PROJECT: AssuranceProfile = AssuranceProfile {
    format_id: "witec-project",
    observe: observe_witec,
    validated: WITEC_PROJECT_VALIDATED,
    confidence: WITEC_PROJECT_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static PERKINELMER_FSM: AssuranceProfile = AssuranceProfile {
    format_id: "perkinelmer-fsm",
    observe: observe_fsm,
    validated: PERKINELMER_FSM_VALIDATED,
    confidence: PERKINELMER_FSM_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static AGILENT_FPA: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-fpa",
    observe: observe_agilent,
    validated: AGILENT_FPA_VALIDATED,
    confidence: AGILENT_FPA_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn common(o: &mut Observations, info: &FileInfo) {
    for t in &info.traces {
        if let Some(d) = a::extra_str(&t.extra, "data_type") {
            o.feature(K::Acquisition, d, &[Scope::Traces]);
        }
        if let Some(i) = a::extra_str(&t.extra, "instrument") {
            o.context(K::Instrument, i);
        }
    }
    if let Some(n) = a::note_with(info, "the file is damaged (") {
        o.undecoded("damaged blocks", &[Scope::Traces], n.to_string());
    }
}

fn observe_opus(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    common(&mut o, info);
    if let Some(v) = info
        .traces
        .iter()
        .find_map(|t| a::extra_str(&t.extra, "software_version"))
        .and_then(|v| a::version_prefix(v, 2))
    {
        o.context(K::Writer, "OPUS");
        o.context(K::WriterVersion, format!("OPUS {v}"));
    }
    for t in &info.traces {
        if let Some(b) = t
            .extra
            .get("block_type")
            .and_then(serde_json::Value::as_array)
        {
            let key: Vec<String> = b.iter().filter_map(a::value_text).collect();
            o.feature(
                K::Record,
                format!("block {}", key.join(".")),
                &[Scope::Traces],
            );
        }
    }
    if let Some(n) = a::note_with(info, "without a data-status block are listed") {
        o.undecoded(
            "data blocks without a data-status block",
            &[],
            n.to_string(),
        );
    }
    o
}

fn observe_omnic(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if a::has_note(info, "OMNIC single-spectrum file (.spa)") {
        o.feature(K::Layout, "spa", &[Scope::Metadata, Scope::Traces]);
    }
    if a::has_note(info, "OMNIC group file (.spg)") {
        o.feature(K::Layout, "spg", &[Scope::Metadata, Scope::Traces]);
    }
    if a::has_note(info, "OMNIC series file (.srs)") {
        o.feature(K::Layout, "srs", &[Scope::Metadata, Scope::Traces]);
    }
    if let Some(n) = a::note_with(info, "series records not decoded") {
        o.undecoded("series profile records", &[], n.to_string());
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(f) = a::extra_str(&t.extra, "final_format") {
            o.feature(
                K::Acquisition,
                format!("final format {f}"),
                &[Scope::Traces],
            );
        }
    }
    o
}

fn observe_wdf(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    match info
        .format_version
        .as_deref()
        .and_then(|v| a::version_prefix(v, 1))
    {
        Some(v) => o.feature(
            K::FormatVersion,
            format!("WiRE {v}"),
            &[Scope::Metadata, Scope::Traces],
        ),
        None => o.feature(
            K::FormatVersion,
            "no WiRE version",
            &[Scope::Metadata, Scope::Traces],
        ),
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(m) = a::extra_str(&t.extra, "measurement_type") {
            o.feature(K::Acquisition, format!("measurement {m}"), &[Scope::Traces]);
        }
        if let Some(s) = a::extra_str(&t.extra, "scan_type") {
            o.feature(K::Acquisition, format!("scan {s}"), &[Scope::Traces]);
        }
        if let Some(x) = a::extra_text(&t.extra, "x_list_type_code") {
            o.feature(K::Layout, format!("x list type {x}"), &[Scope::Traces]);
        }
    }
    for im in &info.images {
        let layout = if im.samples_per_pixel > 1 {
            format!("{}x{}", im.pixel_type.ome_name(), im.samples_per_pixel)
        } else {
            im.pixel_type.ome_name().to_string()
        };
        o.feature(K::SampleLayout, format!("image {layout}"), &[Scope::Pixels]);
    }
    if let Some(n) = a::note_with(info, "map pixels placed in storage order") {
        o.assumed("tables[].x_um/y_um map placement", n.to_string());
    }
    o
}

fn observe_pesp(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(n) = a::note_with(info, "PerkinElmer data set file: ") {
        o.feature(
            K::Layout,
            n.trim_start_matches("PerkinElmer data set file: "),
            &[Scope::Metadata, Scope::Traces],
        );
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(b) = a::extra_str(&t.extra, "beam_type") {
            o.feature(K::Acquisition, format!("beam {b}"), &[Scope::Traces]);
        }
    }
    o
}

fn observe_spc(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(e) = a::extra_str(&t.extra, "value_encoding") {
            o.feature(K::SampleLayout, e, &[Scope::Traces]);
        }
        if t.sweep_count > 1 {
            o.feature(K::Layout, "multifile", &[Scope::Traces]);
        }
        if t.extra
            .get("axis")
            .and_then(|a| a.get("irregular"))
            .is_some()
        {
            o.feature(K::Layout, "x array", &[Scope::Traces]);
        }
        if let Some(s) = a::extra_str(&t.extra, "source_instrument") {
            o.context(K::Writer, s);
        }
    }
    o
}

fn observe_jws(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        // `SPCMAN2 R2.00.00` (compound file), `SPECMAN R2.0.0` / `SPECIRM R2.0.0` (flat)
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
        // a flat file with several channels stores them one after the other
        if (v.starts_with("SPECMAN") || v.starts_with("SPECIRM")) && info.traces.len() > 1 {
            o.feature(
                K::Layout,
                format!("flat, {} channels", info.traces.len()),
                &[Scope::Traces],
            );
        }
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(c) = a::extra_str(&t.extra, "channel_code") {
            o.feature(K::Record, format!("channel {c}"), &[Scope::Traces]);
        }
        if let Some(q) = t
            .extra
            .get("axis")
            .and_then(|x| x.get("quantity"))
            .and_then(serde_json::Value::as_str)
        {
            o.feature(K::Layout, format!("x {q}"), &[Scope::Traces]);
        }
        if t.extra.get("axis").and_then(|x| x.get("irregular"))
            == Some(&serde_json::Value::Bool(true))
        {
            o.feature(K::Layout, "explicit x values", &[Scope::Traces]);
        }
    }
    o
}

fn observe_witec(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            format!("v{v}"),
            &[Scope::Metadata, Scope::Traces, Scope::Pixels],
        );
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(k) = t
            .extra
            .get("x_calibration")
            .and_then(|c| c.get("kind"))
            .and_then(serde_json::Value::as_str)
        {
            o.feature(K::Layout, format!("x calibration {k}"), &[Scope::Traces]);
        }
        if let Some(v) = a::extra_str(&t.extra, "value_type") {
            o.feature(
                K::SampleLayout,
                format!("spectra {v}"),
                &[Scope::Traces, Scope::Pixels],
            );
        }
        if let Some(s) = a::extra_str(&t.extra, "storage_order") {
            o.feature(
                K::Layout,
                format!("spectra {s}"),
                &[Scope::Traces, Scope::Pixels],
            );
        }
        if let Some(q) = t
            .extra
            .get("axis")
            .and_then(|x| x.get("quantity"))
            .and_then(serde_json::Value::as_str)
        {
            o.feature(K::Layout, format!("x {q}"), &[Scope::Traces]);
        }
        if t.extra.contains_key("incomplete_lines") {
            o.feature(
                K::Acquisition,
                "incomplete scan lines",
                &[Scope::Traces, Scope::Pixels],
            );
        }
    }
    for im in &info.images {
        if let Some(k) = a::extra_str(&im.extra, "kind") {
            let order = a::extra_str(&im.extra, "storage_order").unwrap_or("");
            o.feature(
                K::SampleLayout,
                format!("{k} {} {order}", im.pixel_type.ome_name()).trim(),
                &[Scope::Pixels],
            );
        }
    }
    if let Some(n) = a::note_with(info, "not decoded: x is the pixel index") {
        o.undecoded("x calibration", &[Scope::Traces], n.to_string());
    }
    if let Some(n) = a::note_with(info, "is an arbitrary unit") {
        o.undecoded("spectral unit", &[Scope::Traces], n.to_string());
    }
    o
}

fn observe_agilent(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Traces, Scope::Pixels],
        );
    }
    common(&mut o, info);
    for (note, layout) in [
        ("spectra of one tile (.dat)", "tile spectra"),
        ("interferograms of one tile (.seq)", "tile interferograms"),
        ("spectra of the whole mosaic (.dms)", "mosaic spectra"),
        ("spectra of the whole mosaic (.dat)", "mosaic spectra"),
        ("mosaic, spectra of one tile (.dmd)", "mosaic tile spectra"),
        (
            "mosaic, interferograms of one tile (.drd)",
            "mosaic tile interferograms",
        ),
    ] {
        if a::has_note(info, note) {
            o.feature(K::Layout, layout, &[Scope::Traces, Scope::Pixels]);
        }
    }
    for t in &info.traces {
        if let Some(y) = a::extra_str(&t.extra, "y_label") {
            o.feature(K::Acquisition, format!("y {y}"), &[Scope::Traces]);
        }
    }
    if let Some(n) = a::note_with(info, "the x axis is the point index") {
        o.undecoded("x axis", &[Scope::Traces], n.to_string());
    }
    o
}

fn observe_fsm(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(n) = a::note_with(info, "PerkinElmer image file: ") {
        o.feature(
            K::Layout,
            n.trim_start_matches("PerkinElmer image file: "),
            &[Scope::Metadata, Scope::Traces, Scope::Pixels],
        );
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(b) = a::extra_str(&t.extra, "beam_type") {
            o.feature(K::Acquisition, format!("beam {b}"), &[Scope::Traces]);
        }
        if let Some(y) = a::extra_str(&t.extra, "y_units_text") {
            o.feature(K::Acquisition, format!("y {y}"), &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED perkinelmer-fsm (cargo xtask assurance-audit --write; do not edit)
const PERKINELMER_FSM_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const PERKINELMER_FSM_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "INFRARED SPECTRUM", 2, 2, 2),
    a::row(K::Acquisition, "beam Sample", 2, 2, 2),
    a::row(K::Acquisition, "y %T", 2, 2, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 2),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 2),
    a::row(K::Instrument, "Spotlight/Spectrum 3 FT-IR", 1, 1, 1),
    a::row(K::Instrument, "Spotlight/Spectrum One", 1, 1, 1),
    a::row(K::Layout, "DataSet - 4DConst3DInterval", 2, 2, 2),
];
// END GENERATED perkinelmer-fsm

// BEGIN GENERATED agilent-fpa (cargo xtask assurance-audit --write; do not edit)
const AGILENT_FPA_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const AGILENT_FPA_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "INFRARED INTERFEROGRAM", 6, 2, 6),
    a::row(K::Acquisition, "INFRARED SPECTRUM", 7, 2, 7),
    a::row(K::Acquisition, "y Absorbance", 9, 2, 9),
    a::row(K::Acquisition, "y Response", 4, 1, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 13),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 13),
    a::row(K::FormatVersion, "3.4.0.0", 13, 2, 13),
];
// END GENERATED agilent-fpa

// BEGIN GENERATED witec-project (cargo xtask assurance-audit --write; do not edit)
const WITEC_PROJECT_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const WITEC_PROJECT_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "RAMAN SPECTRUM", 12, 9, 12),
    a::row(K::Acquisition, "SPECTRUM", 1, 1, 1),
    a::row(K::Acquisition, "incomplete scan lines", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 9),
    a::row(K::FormatVersion, "v5", 3, 2, 3),
    a::row(K::FormatVersion, "v7", 10, 7, 10),
    a::row(K::FormatVersion, "v8", 1, 1, 1),
    a::row(K::Layout, "spectra column-first", 3, 2, 3),
    a::row(K::Layout, "spectra row-first", 10, 8, 10),
    a::row(K::Layout, "x calibration grating", 10, 6, 10),
    a::row(K::Layout, "x calibration polynomial", 3, 3, 3),
    a::row(K::Layout, "x raman_shift", 12, 9, 12),
    a::row(K::Layout, "x wavelength", 1, 1, 1),
    a::row(K::SampleLayout, "image float column-first", 1, 1, 1),
    a::row(K::SampleLayout, "image float row-first", 1, 1, 1),
    a::row(K::SampleLayout, "image uint8 column-first", 1, 1, 1),
    a::row(K::SampleLayout, "spectra float32", 11, 7, 11),
    a::row(K::SampleLayout, "spectra uint16", 7, 7, 7),
    a::row(K::SampleLayout, "video image uint8", 9, 8, 9),
];
// END GENERATED witec-project

// BEGIN GENERATED jasco-jws (cargo xtask assurance-audit --write; do not edit)
const JASCO_JWS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const JASCO_JWS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "CIRCULAR DICHROISM SPECTRUM", 6, 3, 6),
    a::row(K::Acquisition, "FLUORESCENCE SPECTRUM", 3, 1, 3),
    a::row(K::Acquisition, "INFRARED SPECTRUM", 11, 6, 12),
    a::row(K::Acquisition, "RAMAN SPECTRUM", 1, 1, 1),
    a::row(K::Acquisition, "UV/VIS KINETICS", 1, 1, 1),
    a::row(K::Acquisition, "UV/VIS SPECTRUM", 5, 3, 9),
    a::row(K::Field, "experiment.acquisition.started_at", 4, 2, 28),
    a::row(K::Field, "experiment.instrument.model", 6, 2, 28),
    a::row(K::FormatVersion, "SPCMAN2 R2.00.00", 21, 8, 21),
    a::row(K::FormatVersion, "SPECMAN R2.0.0", 2, 2, 7),
    a::row(K::Layout, "explicit x values", 1, 1, 1),
    a::row(K::Layout, "flat, 2 channels", 1, 1, 1),
    a::row(K::Layout, "x raman_shift", 1, 1, 1),
    a::row(K::Layout, "x time", 1, 1, 1),
    a::row(K::Layout, "x wavelength", 10, 4, 14),
    a::row(K::Layout, "x wavenumber", 11, 6, 12),
    a::row(K::Record, "channel 0x0", 9, 6, 10),
    a::row(K::Record, "channel 0x1001", 6, 3, 6),
    a::row(K::Record, "channel 0x2", 0, 0, 1),
    a::row(K::Record, "channel 0x2001", 6, 3, 6),
    a::row(K::Record, "channel 0x3", 7, 3, 8),
    a::row(K::Record, "channel 0x8", 1, 1, 1),
    a::row(K::Record, "channel 0x9", 0, 0, 1),
    a::row(K::Record, "channel 0xa", 0, 0, 1),
    a::row(K::Record, "channel 0xe", 4, 2, 4),
];
// END GENERATED jasco-jws

// BEGIN GENERATED bruker-opus (cargo xtask assurance-audit --write; do not edit)
const BRUKER_OPUS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BRUKER_OPUS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "INFRARED INTERFEROGRAM", 6, 4, 6),
    a::row(K::Acquisition, "INFRARED SPECTRUM", 25, 12, 26),
    a::row(K::Field, "experiment.acquisition.started_at", 13, 4, 25),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 26),
    a::row(K::FormatVersion, "920622", 25, 12, 26),
    a::row(K::Instrument, "Alpha", 2, 2, 2),
    a::row(K::Instrument, "Alpha II", 4, 2, 4),
    a::row(K::Instrument, "IFS66V/S", 2, 2, 2),
    a::row(K::Instrument, "INVENIO-R", 1, 1, 1),
    a::row(K::Instrument, "Lumos", 1, 1, 1),
    a::row(K::Instrument, "MPA", 1, 1, 1),
    a::row(K::Instrument, "TENSOR 27", 1, 1, 1),
    a::row(K::Instrument, "TENSOR II", 1, 1, 1),
    a::row(K::Instrument, "Tango", 1, 1, 1),
    a::row(K::Instrument, "Tensor 27", 2, 2, 2),
    a::row(K::Instrument, "Tensor II", 1, 1, 1),
    a::row(K::Instrument, "VERTEX", 2, 1, 2),
    a::row(K::Instrument, "VERTEX 70", 1, 1, 1),
    a::row(K::Instrument, "VERTEX 70V", 1, 1, 1),
    a::row(K::Instrument, "VERTEX 80V", 1, 1, 2),
    a::row(K::Instrument, "Vertex 70", 3, 1, 3),
    a::row(K::Record, "block 0.0.0.7.0.2", 0, 0, 1),
    a::row(K::Record, "block 3.1.0.1.0.0", 22, 10, 22),
    a::row(K::Record, "block 3.1.0.1.0.2", 0, 0, 1),
    a::row(K::Record, "block 3.1.0.2.0.0", 6, 4, 6),
    a::row(K::Record, "block 3.1.0.3.0.0", 3, 3, 3),
    a::row(K::Record, "block 3.2.0.1.0.0", 22, 10, 23),
    a::row(K::Record, "block 3.2.0.2.0.0", 6, 4, 6),
    a::row(K::Record, "block 3.3.0.12.0.0", 2, 2, 2),
    a::row(K::Record, "block 3.3.0.22.0.0", 2, 1, 2),
    a::row(K::Record, "block 3.3.0.4.0.0", 21, 9, 21),
    a::row(K::Record, "block 3.3.0.4.0.2", 0, 0, 1),
    a::row(K::Record, "block 3.3.0.4.2.2", 0, 0, 1),
    a::row(K::Record, "block 3.3.0.5.0.0", 2, 2, 2),
    a::row(K::Record, "block 3.3.0.54.0.0", 2, 1, 2),
    a::row(K::Writer, "OPUS", 25, 12, 26),
    a::row(K::WriterVersion, "OPUS 5.5", 4, 3, 4),
    a::row(K::WriterVersion, "OPUS 6.5", 6, 4, 6),
    a::row(K::WriterVersion, "OPUS 7.2", 2, 1, 2),
    a::row(K::WriterVersion, "OPUS 7.5", 3, 2, 3),
    a::row(K::WriterVersion, "OPUS 7.7", 1, 1, 1),
    a::row(K::WriterVersion, "OPUS 7.8", 1, 1, 1),
    a::row(K::WriterVersion, "OPUS 8.0", 0, 0, 1),
    a::row(K::WriterVersion, "OPUS 8.1", 1, 1, 1),
    a::row(K::WriterVersion, "OPUS 8.5", 5, 3, 5),
    a::row(K::WriterVersion, "OPUS 8.7", 2, 2, 2),
];
// END GENERATED bruker-opus

// BEGIN GENERATED thermo-omnic (cargo xtask assurance-audit --write; do not edit)
const THERMO_OMNIC_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const THERMO_OMNIC_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "INFRARED INTERFEROGRAM", 22, 12, 22),
    a::row(K::Acquisition, "INFRARED SPECTRUM", 30, 14, 30),
    a::row(K::Acquisition, "final format %Transmittance", 5, 4, 5),
    a::row(K::Acquisition, "final format Absorbance", 11, 3, 11),
    a::row(K::Acquisition, "final format Volts", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 21, 9, 30),
    a::row(K::Layout, "spa", 22, 13, 22),
    a::row(K::Layout, "spg", 4, 2, 4),
    a::row(K::Layout, "srs", 6, 1, 6),
];
// END GENERATED thermo-omnic

// BEGIN GENERATED renishaw-wdf (cargo xtask assurance-audit --write; do not edit)
const RENISHAW_WDF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const RENISHAW_WDF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "RAMAN SPECTRUM", 14, 6, 14),
    a::row(K::Acquisition, "UNKNOWN", 1, 1, 1),
    a::row(K::Acquisition, "measurement map", 4, 2, 4),
    a::row(K::Acquisition, "measurement series", 1, 1, 1),
    a::row(K::Acquisition, "measurement single", 6, 3, 6),
    a::row(K::Acquisition, "measurement unspecified", 4, 3, 4),
    a::row(K::Acquisition, "scan StreamLine", 2, 2, 2),
    a::row(K::Acquisition, "scan StreamLineHR", 2, 1, 2),
    a::row(K::Acquisition, "scan static", 4, 2, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 13),
    a::row(K::FormatVersion, "WiRE 4", 8, 2, 8),
    a::row(K::FormatVersion, "WiRE 5", 5, 4, 5),
    a::row(K::FormatVersion, "no WiRE version", 2, 2, 2),
    a::row(K::Layout, "x list type 0", 1, 1, 1),
    a::row(K::Layout, "x list type 1", 14, 6, 14),
    a::row(K::SampleLayout, "image float", 3, 2, 3),
    a::row(K::SampleLayout, "image uint8x3", 4, 2, 4),
];
// END GENERATED renishaw-wdf

// BEGIN GENERATED perkinelmer-sp (cargo xtask assurance-audit --write; do not edit)
const PERKINELMER_SP_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const PERKINELMER_SP_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "FLUORESCENCE SPECTRUM", 0, 0, 3),
    a::row(K::Acquisition, "INFRARED SPECTRUM", 11, 9, 11),
    a::row(K::Acquisition, "UV/VIS SPECTRUM", 2, 2, 2),
    a::row(K::Acquisition, "beam Ratio", 9, 7, 9),
    a::row(K::Acquisition, "beam Sample", 1, 1, 1),
    a::row(K::Acquisition, "beam ±ÈÂÊ", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 13),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 13),
    a::row(K::Instrument, "Frontier FT-IR", 3, 3, 3),
    a::row(K::Instrument, "Frontier FT-IR/NIR", 3, 1, 3),
    a::row(K::Instrument, "Lambda 25", 1, 1, 1),
    a::row(K::Instrument, "Lambda 950", 1, 1, 1),
    a::row(K::Instrument, "Spectrum 100", 1, 1, 1),
    a::row(K::Instrument, "Spectrum 3 FT-IR", 1, 1, 1),
    a::row(K::Instrument, "Spectrum One", 1, 1, 1),
    a::row(K::Instrument, "Spectrum Two", 1, 1, 1),
    a::row(K::Instrument, "Spotlight/Spectrum 3 FT-IR", 1, 1, 1),
    a::row(K::Layout, "2D constant interval DataSet file", 13, 11, 13),
];
// END GENERATED perkinelmer-sp

// BEGIN GENERATED galactic-spc (cargo xtask assurance-audit --write; do not edit)
const GALACTIC_SPC_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const GALACTIC_SPC_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "INFRARED SPECTRUM", 9, 7, 9),
    a::row(K::Acquisition, "RAMAN SPECTRUM", 3, 3, 3),
    a::row(K::Acquisition, "UV/VIS SPECTRUM", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 6, 5, 6),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 6),
    a::row(K::FormatVersion, "new (0x4B)", 11, 9, 11),
    a::row(K::FormatVersion, "old (0x4D)", 2, 2, 2),
    a::row(K::Layout, "multifile", 2, 2, 2),
    a::row(K::Layout, "x array", 2, 2, 2),
    a::row(K::SampleLayout, "fixed32", 3, 2, 3),
    a::row(K::SampleLayout, "fixed32-word-swapped", 2, 2, 2),
    a::row(K::SampleLayout, "float32", 8, 7, 8),
    a::row(K::Writer, "Aist-NT", 1, 1, 1),
    a::row(K::Writer, "Digilab", 1, 1, 1),
    a::row(K::Writer, "OMNIC", 3, 2, 3),
    a::row(K::Writer, "WITec", 1, 1, 1),
];
// END GENERATED galactic-spc

pub(crate) static AGILENT_CARY: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-cary",
    observe: observe_cary,
    validated: AGILENT_CARY_VALIDATED,
    confidence: AGILENT_CARY_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe_cary(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        // the application and its version: `Scan 3.00(182)`, `Scan 5.0.0.999`,
        // `Scanning Kinetics 5.0.0.999`
        o.feature(K::Writer, v, &[Scope::Metadata, Scope::Traces]);
    }
    common(&mut o, info);
    for t in &info.traces {
        if let Some(m) = a::extra_str(&t.extra, "y_mode") {
            o.feature(K::Record, format!("y mode {m}"), &[Scope::Traces]);
        }
        if let Some(m) = a::extra_str(&t.extra, "x_mode") {
            o.feature(K::Record, format!("x mode {m}"), &[Scope::Traces]);
        }
        if t.extra.get("axis").and_then(|x| x.get("irregular"))
            == Some(&serde_json::Value::Bool(true))
        {
            o.feature(K::Layout, "explicit x values", &[Scope::Traces]);
        }
        if t.name.as_deref().is_some_and(|n| n.starts_with("baseline")) {
            o.feature(K::Record, "baseline store", &[Scope::Traces]);
        }
    }
    if info
        .notes
        .iter()
        .any(|n| n.starts_with("x unit assumed nm"))
    {
        o.assumed(
            "traces[].extra.axis.unit",
            "nm: the parameter list names no X mode (Scanning Kinetics)",
        );
    }
    if info
        .notes
        .iter()
        .any(|n| n.starts_with("stores listed by `info --view structure`"))
    {
        o.undecoded(
            "graph, report, database and baseline-info stores",
            &[],
            "listed by `info --view structure`, not decoded; the spectra do not depend on them",
        );
    }
    o
}

// BEGIN GENERATED agilent-cary (cargo xtask assurance-audit --write; do not edit)
const AGILENT_CARY_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const AGILENT_CARY_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "UV/VIS SPECTRUM", 20, 5, 24),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 24),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 24),
    a::row(K::Layout, "explicit x values", 0, 0, 4),
    a::row(K::Record, "baseline store", 0, 0, 3),
    a::row(K::Record, "x mode Nanometers", 20, 5, 23),
    a::row(K::Record, "y mode Abs", 20, 5, 24),
    a::row(K::Writer, "Scan 3.00(182)", 20, 5, 20),
    a::row(K::Writer, "Scan 5.0.0.999", 0, 0, 3),
    a::row(K::Writer, "Scanning Kinetics 5.0.0.999", 0, 0, 1),
];
// END GENERATED agilent-cary
