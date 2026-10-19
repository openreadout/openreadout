//! Assurance profiles (`docs/assurance.md`) of the sequencing readers: the variant features of a
//! file and the feature values the development corpus validates. The tables between the
//! GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ILLUMINA_RUN: AssuranceProfile = AssuranceProfile {
    format_id: "illumina-run",
    observe: observe_illumina,
    validated: ILLUMINA_RUN_VALIDATED,
    confidence: ILLUMINA_RUN_CONFIDENCE,
    basis: Basis::VendorDocs,
};

pub(crate) static POD5: AssuranceProfile = AssuranceProfile {
    format_id: "pod5",
    observe: observe_signal,
    validated: POD5_VALIDATED,
    confidence: POD5_CONFIDENCE,
    basis: Basis::VendorDocs,
};

pub(crate) static FAST5: AssuranceProfile = AssuranceProfile {
    format_id: "fast5",
    observe: observe_signal,
    validated: FAST5_VALIDATED,
    confidence: FAST5_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static FASTQ: AssuranceProfile = AssuranceProfile {
    format_id: "fastq",
    observe: observe_reads,
    validated: FASTQ_VALIDATED,
    confidence: FASTQ_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static BAM: AssuranceProfile = AssuranceProfile {
    format_id: "bam",
    observe: observe_reads,
    validated: BAM_VALIDATED,
    confidence: BAM_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static CRAM: AssuranceProfile = AssuranceProfile {
    format_id: "cram",
    observe: observe_cram,
    validated: CRAM_VALIDATED,
    confidence: CRAM_CONFIDENCE,
    basis: Basis::OpenSpec,
};

/// CRAM: the container version and the block codecs decide how records are decoded; where the
/// reference comes from decides how bases are restored.
fn observe_cram(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, format!("CRAM {v}"), &[Scope::Tables]);
    }
    if let Some(t) = info.tables.first() {
        if let Some(c) = t.extra.get("cram_codecs").and_then(|v| v.as_object()) {
            for k in c.keys() {
                o.feature(K::Codec, k, &[Scope::Tables]);
            }
        }
        if let Some(r) = t.extra.get("reference") {
            let n = |k: &str| r.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
            if n("slices_embedded") > 0 {
                o.feature(K::Layout, "reference embedded", &[Scope::Tables]);
            }
            if n("slices_external") > 0 {
                o.feature(K::Layout, "reference external", &[Scope::Tables]);
            }
            if n("containers_reference_not_required") > 0 {
                o.feature(K::Layout, "reference not required", &[Scope::Tables]);
            }
        }
        for k in ["sort_order", "platform"] {
            if let Some(v) = a::extra_str(&t.extra, k) {
                let kind = if k == "platform" {
                    K::Instrument
                } else {
                    K::Layout
                };
                o.feature(kind, format!("{k} {v}"), &[Scope::Tables]);
            }
        }
        if let Some(v) = a::extra_str(&t.extra, "writer") {
            o.feature(K::Writer, format!("writer {v}"), &[]);
        }
    }
    for n in &info.notes {
        if n.contains("reads were not restored") {
            o.undecoded("bases (reference sequence not available)", &[], n.clone());
        }
    }
    o
}

fn observe_illumina(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(t) = info.tables.first() {
        if let Some(v) = t.extra.get("interop_versions").and_then(|v| v.as_object()) {
            for (file, ver) in v {
                // `InterOp/QMetricsOut.bin` → `QMetricsOut v7` (per-cycle copies count once)
                let name = file.rsplit('/').next().unwrap_or(file);
                let name = name.trim_end_matches(".bin");
                o.feature(
                    K::FormatVersion,
                    format!("{name} v{ver}"),
                    &[Scope::Tables, Scope::Traces],
                );
            }
        }
        if let Some(m) = a::extra_str(&t.extra, "instrument_model") {
            o.feature(K::Instrument, m, &[Scope::Metadata]);
        }
        if let Some(v) = a::extra_str(&t.extra, "rta_version") {
            o.feature(
                K::WriterVersion,
                format!("RTA {}", major_minor(v)),
                &[Scope::Tables],
            );
        }
    }
    if let Some(bc) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("base calls"))
        && let Some(s) = a::extra_str(&bc.extra, "storage")
    {
        o.feature(K::Codec, s, &[Scope::Tables]);
    }
    o
}

fn observe_signal(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for t in &info.traces {
        if let Some(c) = a::extra_str(&t.extra, "signal_compression") {
            o.feature(K::Codec, c, &[Scope::Traces]);
        }
        if let Some(c) = a::extra_str(&t.extra, "layout") {
            o.feature(K::Layout, c, &[Scope::Traces]);
        }
    }
    if let Some(t) = info.tables.first()
        && let Some(w) = a::extra_str(&t.extra, "software")
    {
        o.feature(K::Writer, w, &[Scope::Metadata]);
    }
    o
}

fn observe_reads(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(t) = info.tables.first() {
        for k in ["compression", "quality_encoding", "sort_order", "platform"] {
            if let Some(v) = a::extra_str(&t.extra, k) {
                let kind = match k {
                    "compression" => K::Codec,
                    "platform" => K::Instrument,
                    "sort_order" => K::Layout,
                    _ => K::SampleLayout,
                };
                o.feature(kind, format!("{k} {v}"), &[Scope::Tables]);
            }
        }
        // Who wrote the file: the program at the end of a BAM's @PG chain, or the convention
        // of a FASTQ's titles. Records are decoded the same way whoever wrote them.
        for k in ["writer", "title_convention"] {
            if let Some(v) = a::extra_str(&t.extra, k) {
                o.feature(K::Writer, format!("{k} {v}"), &[]);
            }
        }
    }
    // The SAM version of the @HD line: descriptive too, BAM records have kept their layout.
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, format!("SAM {v}"), &[]);
    }
    o
}

fn major_minor(v: &str) -> String {
    let v = v.trim().trim_start_matches(['v', 'V']);
    v.split('.').take(2).collect::<Vec<_>>().join(".")
}

// BEGIN GENERATED illumina-run (cargo xtask assurance-audit --write; do not edit)
const ILLUMINA_RUN_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const ILLUMINA_RUN_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "bcl", 1, 1, 1),
    a::row(K::Codec, "cbcl", 1, 1, 1),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 2),
    a::row(K::FormatVersion, "CorrectedIntMetricsOut v2", 1, 1, 1),
    a::row(K::FormatVersion, "CorrectedIntMetricsOut v4", 1, 1, 1),
    a::row(K::FormatVersion, "ErrorMetricsOut v3", 1, 1, 1),
    a::row(K::FormatVersion, "ExtendedTileMetricsOut v3", 1, 1, 1),
    a::row(K::FormatVersion, "ExtractionMetricsOut v2", 1, 1, 1),
    a::row(K::FormatVersion, "ExtractionMetricsOut v3", 1, 1, 1),
    a::row(K::FormatVersion, "IndexMetricsOut v2", 1, 1, 1),
    a::row(K::FormatVersion, "QMetricsOut v4", 1, 1, 1),
    a::row(K::FormatVersion, "QMetricsOut v7", 1, 1, 1),
    a::row(K::FormatVersion, "TileMetricsOut v2", 1, 1, 1),
    a::row(K::FormatVersion, "TileMetricsOut v3", 1, 1, 1),
    a::row(K::Instrument, "MiSeq", 1, 1, 1),
    a::row(K::Instrument, "NovaSeq 6000", 1, 1, 1),
    a::row(K::WriterVersion, "RTA 3.4", 1, 1, 1),
];
// END GENERATED illumina-run

// BEGIN GENERATED pod5 (cargo xtask assurance-audit --write; do not edit)
const POD5_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const POD5_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "vbz", 6, 3, 6),
    a::row(K::FormatVersion, "0.0.40", 1, 1, 1),
    a::row(K::FormatVersion, "0.3.18", 1, 1, 1),
    a::row(K::FormatVersion, "0.3.30", 1, 1, 1),
    a::row(K::FormatVersion, "0.3.39", 1, 1, 1),
    a::row(K::FormatVersion, "0.3.45", 1, 1, 1),
    a::row(K::FormatVersion, "0.3.46", 1, 1, 1),
    a::row(K::Writer, "MinKNOW 24.02.10 (Bream 7.9.4, Core 5.9.7, Dorado 7.3.9+b2176f600)", 1, 1, 1),
    a::row(K::Writer, "MinKNOW 24.11.8 (Bream 8.2.5, Core 6.2.6, Dorado 7.6.7+de8939544)", 1, 1, 1),
    a::row(K::Writer, "python-pod5-converter", 4, 1, 4),
];
// END GENERATED pod5

// BEGIN GENERATED fast5 (cargo xtask assurance-audit --write; do not edit)
const FAST5_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const FAST5_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "gzip", 2, 1, 2),
    a::row(K::Codec, "vbz", 1, 1, 1),
    a::row(K::FormatVersion, "1.0", 1, 1, 1),
    a::row(K::FormatVersion, "2", 1, 1, 1),
    a::row(K::FormatVersion, "2.0", 1, 1, 1),
    a::row(K::Layout, "multi-read", 3, 2, 3),
    a::row(K::Writer, "MinKNOW 3.4.0-rc3", 1, 1, 1),
    a::row(K::Writer, "MinKNOW 3.6.0-rc2", 2, 2, 2),
];
// END GENERATED fast5

// BEGIN GENERATED fastq (cargo xtask assurance-audit --write; do not edit)
const FASTQ_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const FASTQ_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "compression gzip", 4, 3, 4),
    a::row(K::Codec, "compression none", 3, 3, 3),
    a::row(K::SampleLayout, "quality_encoding phred+33", 7, 5, 7),
    a::row(K::Writer, "title_convention illumina 1.8+", 1, 1, 1),
    a::row(K::Writer, "title_convention nanopore", 1, 1, 1),
    a::row(K::Writer, "title_convention nanopore minknow", 1, 1, 1),
    a::row(K::Writer, "title_convention nanopore sam tags", 1, 1, 1),
    a::row(K::Writer, "title_convention sra", 1, 1, 1),
];
// END GENERATED fastq

// BEGIN GENERATED bam (cargo xtask assurance-audit --write; do not edit)
const BAM_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BAM_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "compression bgzf", 11, 5, 11),
    a::row(K::FormatVersion, "SAM 1.0", 1, 1, 1),
    a::row(K::FormatVersion, "SAM 1.4", 2, 2, 2),
    a::row(K::FormatVersion, "SAM 1.5", 5, 3, 5),
    a::row(K::FormatVersion, "SAM 1.6", 2, 2, 2),
    a::row(K::Instrument, "platform ILLUMINA", 4, 3, 4),
    a::row(K::Instrument, "platform PACBIO", 5, 3, 5),
    a::row(K::Layout, "sort_order coordinate", 5, 5, 5),
    a::row(K::Layout, "sort_order queryname", 1, 1, 1),
    a::row(K::Layout, "sort_order sorted", 1, 1, 1),
    a::row(K::Layout, "sort_order unknown", 3, 2, 3),
    a::row(K::Writer, "writer TopHat", 1, 1, 1),
    a::row(K::Writer, "writer bax2bam", 1, 1, 1),
    a::row(K::Writer, "writer bwa", 1, 1, 1),
    a::row(K::Writer, "writer ccs", 1, 1, 1),
    a::row(K::Writer, "writer minimap2", 1, 1, 1),
    a::row(K::Writer, "writer samtools", 2, 2, 2),
    a::row(K::Writer, "writer scramble", 1, 1, 1),
    a::row(K::Writer, "writer update_bam.py", 2, 1, 2),
];
// END GENERATED bam

// BEGIN GENERATED cram (cargo xtask assurance-audit --write; do not edit)
const CRAM_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const CRAM_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "arith", 1, 1, 1),
    a::row(K::Codec, "bzip2", 2, 1, 3),
    a::row(K::Codec, "fqzcomp", 1, 1, 1),
    a::row(K::Codec, "gzip", 29, 13, 30),
    a::row(K::Codec, "lzma", 2, 1, 2),
    a::row(K::Codec, "rans4x8", 20, 11, 20),
    a::row(K::Codec, "ransNx16", 4, 3, 5),
    a::row(K::Codec, "raw", 29, 11, 30),
    a::row(K::Codec, "tok3", 2, 1, 2),
    a::row(K::FormatVersion, "CRAM 2.0", 1, 1, 1),
    a::row(K::FormatVersion, "CRAM 2.1", 1, 1, 1),
    a::row(K::FormatVersion, "CRAM 3.0", 28, 12, 28),
    a::row(K::FormatVersion, "CRAM 3.1", 4, 3, 5),
    a::row(K::Instrument, "platform 10X", 1, 1, 1),
    a::row(K::Instrument, "platform ILLUMINA", 7, 6, 8),
    a::row(K::Instrument, "platform ONT", 1, 1, 1),
    a::row(K::Instrument, "platform PACBIO", 1, 1, 1),
    a::row(K::Layout, "reference embedded", 5, 2, 6),
    a::row(K::Layout, "reference external", 26, 13, 26),
    a::row(K::Layout, "reference not required", 8, 3, 9),
    a::row(K::Layout, "sort_order coordinate", 15, 11, 16),
    a::row(K::Layout, "sort_order unsorted", 2, 2, 2),
    a::row(K::Writer, "writer Hey!", 1, 1, 1),
    a::row(K::Writer, "writer MarkDuplicates", 1, 1, 1),
    a::row(K::Writer, "writer bwa", 1, 1, 1),
    a::row(K::Writer, "writer cow", 1, 1, 1),
    a::row(K::Writer, "writer cramtools", 2, 1, 2),
    a::row(K::Writer, "writer longranger", 1, 1, 1),
    a::row(K::Writer, "writer sambamba-6DEDF075", 1, 1, 1),
    a::row(K::Writer, "writer samtools", 12, 6, 13),
    a::row(K::Writer, "writer scramble", 4, 2, 4),
];
// END GENERATED cram
