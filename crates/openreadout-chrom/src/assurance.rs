//! Assurance profiles (`docs/assurance.md`) of the chromatography readers (Agilent ChemStation
//! and OpenLab CDS, ANDI netCDF, Shimadzu LabSolutions): the variant features of a data set and
//! the feature values the development corpus validates. The tables between the GENERATED
//! markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static CHEMSTATION: AssuranceProfile = AssuranceProfile {
    format_id: "chemstation",
    observe: observe_chemstation,
    validated: CHEMSTATION_VALIDATED,
    confidence: CHEMSTATION_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static OPENLAB_CDS: AssuranceProfile = AssuranceProfile {
    format_id: "openlab-cds",
    observe: observe_openlab,
    validated: OPENLAB_CDS_VALIDATED,
    confidence: OPENLAB_CDS_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static ANDI_CHROM: AssuranceProfile = AssuranceProfile {
    format_id: "andi-chrom",
    observe: observe_andi,
    validated: ANDI_CHROM_VALIDATED,
    confidence: ANDI_CHROM_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static EMPOWER_ARW: AssuranceProfile = AssuranceProfile {
    format_id: "empower-arw",
    observe: observe_empower_arw,
    validated: EMPOWER_ARW_VALIDATED,
    confidence: EMPOWER_ARW_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// An Empower ASCII export: the layout (2D, regular or not) and the header fields exported.
fn observe_empower_arw(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        let regular = t.sample_rate_hz > 0.0;
        o.feature(
            K::Layout,
            if regular {
                "2D, evenly spaced times"
            } else {
                "2D, uneven times"
            },
            &[Scope::Traces],
        );
        if let Some(e) = a::extra_str(&t.extra, "line_ending") {
            o.feature(K::Dialect, format!("{e} line endings"), &[Scope::Traces]);
        }
    }
    o.assumed(
        "traces[].channels[].unit",
        "the export does not state the value's unit; it is left unset",
    );
    o
}

pub(crate) static SHIMADZU: AssuranceProfile = AssuranceProfile {
    format_id: "shimadzu",
    observe: observe_shimadzu,
    validated: SHIMADZU_VALIDATED,
    confidence: SHIMADZU_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static CHROMELEON: AssuranceProfile = AssuranceProfile {
    format_id: "chromeleon",
    observe: observe_chromeleon,
    validated: CHROMELEON_VALIDATED,
    confidence: CHROMELEON_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe_chromeleon(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = info
        .format_version
        .as_deref()
        .and_then(|v| a::version_prefix(v, 2))
    {
        o.feature(
            K::WriterVersion,
            format!("Chromeleon {v}"),
            &[Scope::Metadata, Scope::Traces],
        );
    }
    for t in &info.traces {
        if let Some(e) = a::extra_str(&t.extra, "encoding") {
            o.feature(K::Codec, e, &[Scope::Traces]);
        }
        // the rate and unit go with a block header's scale and grid
        let unit = t
            .channels
            .iter()
            .find(|c| c.name != "time")
            .and_then(|c| c.unit.as_deref())
            .unwrap_or("no unit");
        let kind = if t.extra.contains_key("spectral_field") {
            "3D field"
        } else {
            "signal"
        };
        if t.sample_rate_hz > 0.0 {
            o.feature(
                K::Layout,
                format!("{} Hz {kind} in {unit}", t.sample_rate_hz),
                &[Scope::Traces],
            );
        } else {
            o.feature(
                K::Layout,
                format!("irregular {kind} in {unit}"),
                &[Scope::Traces],
            );
        }
        if let Some(m) = a::extra_str(&t.extra, "module") {
            o.context(K::Instrument, m);
        }
    }
    if !info.traces.is_empty() {
        o.calibration(
            "stored integers to signal units",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "trace values = summed differences × the block header's scale (numerator / denominator), equal to the sequence file's scale factor",
        );
    }
    for t in info
        .tables
        .iter()
        .filter(|t| t.name.as_deref() == Some("vendor_peaks"))
    {
        for v in t
            .extra
            .get("results_versions")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            o.feature(
                K::FormatVersion,
                format!("stored results {v}"),
                &[Scope::Tables],
            );
        }
    }
    if let Some(n) = a::note_with(info, "stored results of") {
        o.undecoded(
            "stored integration results",
            &[Scope::Tables],
            n.to_string(),
        );
    }
    if let Some(n) = a::note_with(info, "mass-spectrometry data is an embedded Thermo .raw") {
        o.undecoded("embedded MS data", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "some signals have no description") {
        o.undecoded("signal units", &[Scope::Traces], n.to_string());
    }
    o
}

fn observe_chemstation(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(v) = a::extra_text(&t.extra, "format_version") {
            o.feature(
                K::FormatVersion,
                format!("signal {v}"),
                &[Scope::Metadata, Scope::Traces],
            );
        }
        if let Some(e) = a::extra_str(&t.extra, "encoding") {
            o.feature(K::Codec, e, &[Scope::Traces]);
        }
        if let Some(f) = a::extra_str(&t.extra, "file_type") {
            o.context(K::Acquisition, f.to_ascii_uppercase());
        }
        if let Some(i) = a::extra_str(&t.extra, "instrument") {
            o.context(K::Instrument, i);
        }
        if let Some(s) = a::extra_str(&t.extra, "software").and_then(|s| a::version_prefix(s, 1)) {
            o.context(K::WriterVersion, format!("ChemStation {s}"));
        }
    }
    for s in &info.spectra {
        if let Some(v) = a::extra_text(&s.extra, "format_version") {
            o.feature(
                K::FormatVersion,
                format!("MS {v}"),
                &[Scope::Metadata, Scope::Spectra],
            );
        }
        if let Some(e) = a::extra_str(&s.extra, "encoding") {
            o.feature(K::Codec, e, &[Scope::Spectra]);
        }
        if let Some(f) = a::extra_str(&s.extra, "file_type") {
            o.context(K::Acquisition, f.to_ascii_uppercase());
        }
    }
    if !info.traces.is_empty() {
        o.calibration(
            "stored integers to signal units",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "trace values = raw × scale + offset with the factors from each file's header, as ChemStation reports them",
        );
    }
    if let Some(n) = a::note_with(info, "listed but not decoded (unknown version)") {
        o.undecoded("signal files of unknown versions", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "a signal body ends early") {
        o.undecoded("truncated signal body", &[Scope::Traces], n.to_string());
    }
    o
}

fn observe_openlab(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            format!("signal {v}"),
            &[Scope::Metadata, Scope::Traces],
        );
    }
    for t in &info.traces {
        if let Some(e) = a::extra_str(&t.extra, "encoding") {
            o.feature(K::Codec, e, &[Scope::Traces]);
        }
        if let Some(i) = a::extra_str(&t.extra, "instrument") {
            o.context(K::Instrument, i);
        }
        if let Some(v) =
            a::extra_str(&t.extra, "software_version").and_then(|v| a::version_prefix(v, 2))
        {
            o.context(K::WriterVersion, format!("OpenLab CDS {v}"));
        }
    }
    for t in &info.tables {
        if let Some(s) = a::extra_str(&t.extra, "software") {
            o.feature(K::Writer, a::writer_name_only(s), &[Scope::Tables]);
        }
        o.feature(K::Record, "vendor results (.rx)", &[Scope::Tables]);
    }
    if !info.traces.is_empty() {
        o.calibration(
            "stored integers to signal units",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "trace values = raw × scale with the factor from each part's header",
        );
    }
    if let Some(n) = a::note_with(info, "the result package was not read") {
        o.undecoded("result package", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "the container has no part for") {
        o.undecoded("signals without a part", &[], n.to_string());
    }
    if let Some(n) = a::note_with(
        info,
        "signal parts hold fewer values than the manifest declares",
    ) {
        o.undecoded("short signal parts", &[Scope::Traces], n.to_string());
    }
    o
}

fn observe_andi(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    match &info.format_version {
        Some(v) => o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Traces, Scope::Spectra],
        ),
        None => o.feature(
            K::FormatVersion,
            "no template revision",
            &[Scope::Metadata, Scope::Traces, Scope::Spectra],
        ),
    }
    if info.spectra.is_empty() {
        o.feature(K::Layout, "chromatography template", &[Scope::Traces]);
    } else {
        o.feature(
            K::Layout,
            "mass-spectrometry template",
            &[Scope::Spectra, Scope::Traces],
        );
    }
    for t in &info.traces {
        for c in &t.channels {
            if let Some(u) = &c.unit {
                o.context(K::Record, format!("unit {u}"));
            }
        }
    }
    if let Some(n) = a::note_with(info, "netCDF file without ANDI template attributes") {
        o.feature(
            K::Dialect,
            "netCDF without ANDI attributes",
            &[Scope::Metadata],
        );
        o.undecoded("unmapped netCDF variables", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "the scan variables could not be read") {
        o.undecoded("scan variables", &[], n.to_string());
    }
    o
}

fn observe_shimadzu(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for t in &info.traces {
        if let Some(d) = a::extra_str(&t.extra, "detector_model") {
            o.context(K::Instrument, d);
        }
        // what a trace is and which layout stores it: chromatogram scaling differs between the
        // older (`LC Raw Data`) and newer (`LSS Raw Data`) layouts, f64 records are the GC's
        let storage = a::extra_str(&t.extra, "stream")
            .and_then(|s| s.split('/').next())
            .unwrap_or("");
        let kind = match a::extra_str(&t.extra, "detector") {
            Some("status") => "status trace",
            Some("pda") => "pda spectra",
            Some("pda_max_plot") => "pda max plot",
            _ if a::extra_str(&t.extra, "stored_as") == Some("f64") => "chromatogram (f64)",
            _ => "chromatogram",
        };
        o.feature(K::Layout, format!("{kind} in {storage}"), &[Scope::Traces]);
    }
    if info
        .tables
        .iter()
        .any(|t| t.name.as_deref() == Some("vendor_peaks"))
    {
        o.feature(K::Record, "vendor peak table", &[Scope::Tables]);
    }
    for s in &info.spectra {
        if let Some(k) = s
            .extra
            .get("acquisitions")
            .and_then(serde_json::Value::as_object)
        {
            for (name, _) in k {
                if name == "mrm" || name == "sim" {
                    o.feature(K::Acquisition, format!("LC-MS {name}"), &[Scope::Spectra]);
                } else {
                    o.context(K::Acquisition, format!("LC-MS {name}"));
                }
            }
        }
    }
    if let Some(n) = a::note_with(info, "full-scan or product-ion-scan records are listed") {
        o.undecoded(
            "LC-MS profile spectra (full and product-ion scans)",
            &[],
            n.to_string(),
        );
    }
    if let Some(n) = a::note_with(info, "values are the detector's stored integers") {
        o.calibration(
            "detector integers to signal units",
            CalibrationStatus::NotApplied,
            &[Scope::Traces],
            format!("{n}; LabSolutions reports these traces (and peak areas) in µV or mAU"),
        );
    }
    if let Some(n) = a::note_with(info, "no decodable signal stream") {
        o.undecoded("raw-data storages", &[], n.to_string());
    }
    o
}

// BEGIN GENERATED chemstation (cargo xtask assurance-audit --write; do not edit)
const CHEMSTATION_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const CHEMSTATION_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "GC / MS DATA FILE", 14, 2, 14),
    a::row(K::Acquisition, "GC DATA FILE", 9, 4, 9),
    a::row(K::Acquisition, "LC DATA FILE", 5, 4, 5),
    a::row(K::Acquisition, "MSD SPECTRAL FILE", 2, 2, 2),
    a::row(K::Codec, "delta_records", 4, 4, 4),
    a::row(K::Codec, "float64", 2, 2, 2),
    a::row(K::Codec, "mass_spectrum_records", 16, 4, 16),
    a::row(K::Codec, "second_difference", 7, 4, 7),
    a::row(K::Codec, "spectrum_records", 2, 2, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 10, 5, 28),
    a::row(K::Field, "experiment.instrument.model", 5, 4, 21),
    a::row(K::FormatVersion, "MS 2", 16, 4, 16),
    a::row(K::FormatVersion, "signal 130", 2, 2, 2),
    a::row(K::FormatVersion, "signal 131", 2, 2, 2),
    a::row(K::FormatVersion, "signal 179", 2, 2, 2),
    a::row(K::FormatVersion, "signal 181", 5, 2, 5),
    a::row(K::FormatVersion, "signal 30", 2, 2, 2),
    a::row(K::FormatVersion, "signal 81", 2, 2, 2),
    a::row(K::Instrument, "DAD1", 1, 1, 1),
    a::row(K::Instrument, "G1315B", 2, 2, 2),
    a::row(K::Instrument, "G1365B", 1, 1, 1),
    a::row(K::Instrument, "GCI", 9, 5, 9),
    a::row(K::Instrument, "HP G1530A", 2, 2, 2),
    a::row(K::WriterVersion, "ChemStation 1", 2, 2, 2),
    a::row(K::WriterVersion, "ChemStation 3", 4, 1, 4),
    a::row(K::WriterVersion, "ChemStation 5", 1, 1, 1),
    a::row(K::WriterVersion, "ChemStation 6", 1, 1, 1),
    a::row(K::WriterVersion, "ChemStation 7", 1, 1, 1),
];
// END GENERATED chemstation

// BEGIN GENERATED openlab-cds (cargo xtask assurance-audit --write; do not edit)
const OPENLAB_CDS_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const OPENLAB_CDS_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "InstrumentTrace179", 6, 2, 6),
    a::row(K::Codec, "Signal179", 12, 3, 12),
    a::row(K::Codec, "Spectra131", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 12),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 4),
    a::row(K::FormatVersion, "signal 131", 2, 1, 2),
    a::row(K::FormatVersion, "signal 179", 10, 3, 10),
    a::row(K::Instrument, "G1364F", 1, 1, 1),
    a::row(K::Instrument, "G7104C", 1, 1, 1),
    a::row(K::Instrument, "G7111B", 3, 1, 3),
    a::row(K::Instrument, "G7115A", 1, 1, 1),
    a::row(K::Instrument, "G7116A", 3, 2, 3),
    a::row(K::Instrument, "G7117C", 3, 1, 3),
    a::row(K::Instrument, "G7121A", 2, 1, 2),
    a::row(K::Instrument, "G7167A", 3, 2, 3),
    a::row(K::Record, "vendor results (.rx)", 9, 2, 10),
    a::row(K::Writer, "Agilent OpenLAB Data Analysis Processing Server", 6, 1, 6),
    a::row(K::Writer, "Agilent OpenLab Data Analysis", 3, 1, 3),
    a::row(K::WriterVersion, "OpenLab CDS 2.5", 3, 1, 3),
    a::row(K::WriterVersion, "OpenLab CDS 2.8", 1, 1, 1),
];
// END GENERATED openlab-cds

// BEGIN GENERATED empower-arw (cargo xtask assurance-audit --write; do not edit)
const EMPOWER_ARW_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const EMPOWER_ARW_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "CR line endings", 6, 1, 6),
    a::row(K::Layout, "2D, evenly spaced times", 6, 1, 6),
];
// END GENERATED empower-arw

// BEGIN GENERATED andi-chrom (cargo xtask assurance-audit --write; do not edit)
const ANDI_CHROM_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const ANDI_CHROM_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 24, 4, 24),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 2),
    a::row(K::FormatVersion, "1", 1, 1, 1),
    a::row(K::FormatVersion, "1.0", 2, 2, 2),
    a::row(K::FormatVersion, "1.0.1", 22, 3, 22),
    a::row(K::Layout, "chromatography template", 4, 4, 23),
    a::row(K::Layout, "mass-spectrometry template", 2, 2, 2),
    a::row(K::Record, "unit Arbitrary Intensity Units", 2, 2, 2),
    a::row(K::Record, "unit Volts", 20, 1, 20),
    a::row(K::Record, "unit au", 1, 1, 1),
    a::row(K::Record, "unit mAU", 1, 1, 1),
    a::row(K::Record, "unit mVolts", 1, 1, 1),
];
// END GENERATED andi-chrom

// BEGIN GENERATED shimadzu (cargo xtask assurance-audit --write; do not edit)
const SHIMADZU_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const SHIMADZU_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "LC-MS full scan", 1, 1, 1),
    a::row(K::Acquisition, "LC-MS mrm", 2, 1, 6),
    a::row(K::Acquisition, "LC-MS product ion scan", 2, 1, 2),
    a::row(K::Acquisition, "LC-MS sim", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 12),
    a::row(K::FormatVersion, "3.00", 2, 2, 2),
    a::row(K::FormatVersion, "5.01", 7, 3, 10),
    a::row(K::Instrument, "SFID1", 1, 1, 1),
    a::row(K::Instrument, "SPD-20A", 1, 1, 3),
    a::row(K::Instrument, "SPD-20AV", 1, 1, 1),
    a::row(K::Layout, "chromatogram (f64) in LSS Raw Data", 1, 1, 1),
    a::row(K::Layout, "chromatogram in LC Raw Data", 2, 2, 2),
    a::row(K::Layout, "chromatogram in LSS Raw Data", 0, 0, 2),
    a::row(K::Layout, "pda max plot in PDA 3D Raw Data", 2, 2, 2),
    a::row(K::Layout, "pda spectra in PDA 3D Raw Data", 2, 2, 2),
    a::row(K::Layout, "status trace in LC Raw Data", 2, 2, 2),
    a::row(K::Layout, "status trace in LSS Raw Data", 6, 3, 9),
    a::row(K::Record, "vendor peak table", 1, 1, 5),
];
// END GENERATED shimadzu

// BEGIN GENERATED chromeleon (cargo xtask assurance-audit --write; do not edit)
const CHROMELEON_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CHROMELEON_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "3DRawSpc", 2, 2, 2),
    a::row(K::Codec, "PtsDDCmp", 1, 1, 1),
    a::row(K::Codec, "PtsLDiff", 9, 5, 11),
    a::row(K::Codec, "PtsLL2Df", 1, 1, 1),
    a::row(K::FormatVersion, "stored results 1", 1, 1, 7),
    a::row(K::FormatVersion, "stored results 2", 0, 0, 3),
    a::row(K::Instrument, "Acquity.dll", 0, 0, 1),
    a::row(K::Instrument, "DAD3000.dll", 0, 0, 1),
    a::row(K::Instrument, "DC-6000", 1, 1, 2),
    a::row(K::Instrument, "ICS-6000 SP", 1, 1, 2),
    a::row(K::Instrument, "PumpLPG3X00RS.dll", 0, 0, 1),
    a::row(K::Instrument, "Thermo Scientific Trace GC", 1, 1, 7),
    a::row(K::Instrument, "Thermo.MassSpectrometer", 0, 0, 1),
    a::row(K::Layout, "1 Hz signal in mL/min", 1, 1, 1),
    a::row(K::Layout, "10 Hz signal in bar", 1, 1, 1),
    a::row(K::Layout, "100 Hz signal in bar", 1, 1, 1),
    a::row(K::Layout, "16 Hz signal in psi", 2, 1, 2),
    a::row(K::Layout, "2 Hz signal in nC", 2, 1, 2),
    a::row(K::Layout, "20 Hz 3D field in mAU", 1, 1, 1),
    a::row(K::Layout, "20 Hz signal in mAU", 1, 1, 1),
    a::row(K::Layout, "25 Hz 3D field in mAU", 1, 1, 1),
    a::row(K::Layout, "25 Hz signal in mAU", 1, 1, 1),
    a::row(K::Layout, "50 Hz signal in mV", 5, 2, 7),
    a::row(K::Layout, "irregular signal in counts", 1, 1, 1),
    a::row(K::WriterVersion, "Chromeleon 7.2", 7, 4, 10),
    a::row(K::WriterVersion, "Chromeleon 7.3", 2, 1, 2),
];
// END GENERATED chromeleon
