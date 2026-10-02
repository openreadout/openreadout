//! Assurance profile (`docs/assurance.md`): the variant features of an ND2 file and the feature
//! values the development corpus validates. The table between the GENERATED markers is written
//! by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ND2: AssuranceProfile = AssuranceProfile {
    format_id: "nd2",
    observe,
    validated: ND2_VALIDATED,
    confidence: ND2_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    a::writer_context(&mut o, info);
    for s in a::extra_values(info, "stored_sample_order") {
        o.feature(K::SampleLayout, format!("stored {s}"), &[Scope::Pixels]);
    }
    for im in &info.images {
        if let Some(loops) = im.extra.get("loops").and_then(serde_json::Value::as_array) {
            for l in loops {
                if let Some(k) = l.get("kind").and_then(serde_json::Value::as_str) {
                    // Loops map frames to (c, z, t, position): they decide which plane is which.
                    o.feature(
                        K::Layout,
                        format!("loop {k}"),
                        &[Scope::Pixels, Scope::Metadata],
                    );
                }
            }
        }
    }
    if info.images.len() > 1 {
        o.feature(K::Layout, "multi_position", &[Scope::Pixels]);
    }
    if a::has_note(info, "container index was rebuilt by scanning") {
        o.undecoded(
            "damaged container index",
            &[Scope::Pixels, Scope::Metadata],
            "the chunk map was rebuilt by scanning the file; `check` lists what was recovered",
        );
    }
    if let Some(n) = a::note_with(info, "it is folded into T") {
        o.feature(K::Layout, "custom_loop", &[Scope::Pixels]);
        o.assumed("images[].size_t", n.to_string());
    }
    if let Some(n) = a::note_with(info, "the extra frames are not addressed") {
        o.undecoded("frames beyond the loop tree", &[], n.to_string());
    }
    for n in info.notes.iter().filter(|n| n.starts_with("structure: ")) {
        o.undecoded(
            n.trim_start_matches("structure: "),
            &[Scope::Pixels],
            "a structural problem the reader worked around; `check` gives the detail",
        );
    }
    o
}

/// What only the dataset knows: the frame codec (`eCompression`).
pub(crate) fn internal(compression: Option<i64>, legacy: bool) -> Observations {
    let mut o = Observations::default();
    let codec = if legacy {
        "jpeg2000".to_string()
    } else {
        match compression.unwrap_or(2) {
            2 => "uncompressed".into(),
            0 => "zlib".into(),
            1 => "lossy".into(),
            n => format!("unknown({n})"),
        }
    };
    o.feature(K::Codec, &codec, &[Scope::Pixels]);
    if codec == "lossy" {
        o.undecoded(
            "lossy-compressed frames",
            &[],
            "no public sample to derive the codec from: reading planes exits 6",
        );
    }
    o
}

// BEGIN GENERATED nd2 (cargo xtask assurance-audit --write; do not edit)
const ND2_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const ND2_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "jpeg2000", 5, 2, 5),
    a::row(K::Codec, "uncompressed", 20, 7, 21),
    a::row(K::Codec, "zlib", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 24, 7, 24),
    a::row(K::FormatVersion, "2.0", 2, 1, 2),
    a::row(K::FormatVersion, "2.1", 3, 2, 4),
    a::row(K::FormatVersion, "3.0", 17, 6, 17),
    a::row(K::FormatVersion, "legacy-jp2", 5, 2, 5),
    a::row(K::Layout, "loop ne_time_loop", 10, 4, 10),
    a::row(K::Layout, "loop time_loop", 9, 4, 10),
    a::row(K::Layout, "loop xy_position_loop", 11, 4, 11),
    a::row(K::Layout, "loop z_stack_loop", 12, 4, 13),
    a::row(K::Layout, "multi_position", 9, 3, 9),
    a::row(K::SampleLayout, "stored BGR", 3, 2, 3),
    a::row(K::SampleLayout, "stored RGB", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 22, 6, 23),
    a::row(K::SampleLayout, "uint8", 1, 1, 1),
    a::row(K::SampleLayout, "uint8x3", 4, 3, 4),
    a::row(K::Writer, "NIS-Elements", 27, 7, 28),
    a::row(K::WriterVersion, "NIS-Elements 2.30", 2, 1, 2),
    a::row(K::WriterVersion, "NIS-Elements 3.0", 3, 2, 4),
    a::row(K::WriterVersion, "NIS-Elements 3.20", 3, 1, 3),
    a::row(K::WriterVersion, "NIS-Elements 4.13", 1, 1, 1),
    a::row(K::WriterVersion, "NIS-Elements 4.50", 1, 1, 1),
    a::row(K::WriterVersion, "NIS-Elements 4.51", 1, 1, 1),
    a::row(K::WriterVersion, "NIS-Elements 5.20", 7, 1, 7),
    a::row(K::WriterVersion, "NIS-Elements 5.30", 1, 1, 1),
    a::row(K::WriterVersion, "NIS-Elements 5.42", 3, 2, 3),
];
// END GENERATED nd2
