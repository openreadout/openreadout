//! Assurance profile (`docs/assurance.md`): the variant features of a Leica LIF file (and its
//! XLEF/LOF/LIFEXT family) and the feature values the development corpus validates. The table
//! between the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static LIF: AssuranceProfile = AssuranceProfile {
    format_id: "lif",
    observe,
    validated: LIF_VALIDATED,
    confidence: LIF_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for im in &info.images {
        if let Some(i) = &im.instrument {
            if let Some(sw) = &i.software {
                // "LAS X [ BETA ]" is LAS X.
                let name = sw.split('[').next().unwrap_or(sw).trim();
                o.feature(K::Writer, name, &[Scope::Metadata]);
                if let Some(v) = i
                    .software_version
                    .as_deref()
                    .and_then(|v| a::version_prefix(v, 1))
                {
                    o.context(K::WriterVersion, format!("{name} {v}"));
                }
            }
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
        }
        if im.mosaic.as_ref().is_some_and(|m| m.tile_count > 1) {
            o.feature(K::Layout, "tile_scan", &[Scope::Pixels]);
        }
        if im.extra.contains_key("lambda") {
            o.feature(
                K::Acquisition,
                "lambda_scan",
                &[Scope::Metadata, Scope::Pixels],
            );
        }
        if im.pyramid_levels > 1 {
            o.feature(K::Layout, "pyramid", &[Scope::Pixels]);
        }
        if let Some(f) = im.extra.get("flim") {
            o.feature(K::Acquisition, "flim", &[Scope::Pixels]);
            if f.get("decoded").and_then(serde_json::Value::as_bool) == Some(false) {
                o.undecoded(
                    "FALCON FLIM/TCSPC photon data",
                    &[],
                    "geometry is reported; reading these planes exits 6, the derived intensity/lifetime images are readable",
                );
            }
        }
    }
    // XLIF frames stored as image files: which file types.
    if let Some(n) = a::note_with(info, "pixels come from the XLIF's frame image files (")
        && let Some(kinds) = n
            .split_once("files (")
            .and_then(|(_, r)| r.split_once(')'))
            .map(|(k, _)| k)
    {
        for k in kinds.split(", ") {
            o.feature(K::Codec, format!("xlif frame {k}"), &[Scope::Pixels]);
        }
    }
    if a::has_note(info, "LIFEXT sidecar") {
        o.feature(K::Layout, "lifext_sidecar", &[Scope::Metadata]);
    }
    if a::has_note(info, "file appears truncated") {
        o.undecoded(
            "truncated memory blocks",
            &[Scope::Pixels],
            "the file ends before some memory blocks; `check` lists the planes affected",
        );
    }
    o
}

// BEGIN GENERATED lif (cargo xtask assurance-audit --write; do not edit)
const LIF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const LIF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "flim", 2, 2, 2),
    a::row(K::Acquisition, "lambda_scan", 1, 1, 1),
    a::row(K::Codec, "xlif frame tiff", 4, 2, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 37, 29, 56),
    a::row(K::Field, "experiment.instrument.model", 12, 7, 56),
    a::row(K::FormatVersion, "1", 1, 1, 1),
    a::row(K::FormatVersion, "2", 53, 43, 56),
    a::row(K::Instrument, "DM4000B-CA", 1, 1, 1),
    a::row(K::Instrument, "DM4000B-M", 3, 1, 3),
    a::row(K::Instrument, "DM6000B", 1, 1, 1),
    a::row(K::Instrument, "DM6000B-CS", 1, 1, 1),
    a::row(K::Instrument, "DM6B", 1, 1, 1),
    a::row(K::Instrument, "DM6B-Z-CFS", 1, 1, 1),
    a::row(K::Instrument, "DM6B-Z-CS", 2, 2, 2),
    a::row(K::Instrument, "DMI6000B-CS", 6, 6, 6),
    a::row(K::Instrument, "DMI8", 4, 3, 5),
    a::row(K::Instrument, "DMI8-CS", 15, 14, 15),
    a::row(K::Instrument, "DMIL", 1, 1, 1),
    a::row(K::Instrument, "SIMULATOR", 1, 1, 1),
    a::row(K::Instrument, "TCS SP5", 15, 10, 17),
    a::row(K::Instrument, "TCS SPE", 1, 1, 1),
    a::row(K::Layout, "lifext_sidecar", 1, 1, 1),
    a::row(K::Layout, "tile_scan", 6, 5, 6),
    a::row(K::SampleLayout, "float", 2, 2, 2),
    a::row(K::SampleLayout, "uint16", 14, 13, 17),
    a::row(K::SampleLayout, "uint32", 3, 3, 3),
    a::row(K::SampleLayout, "uint8", 43, 35, 43),
    a::row(K::Writer, "LAS X", 35, 30, 36),
    a::row(K::Writer, "LAS-AF", 1, 1, 1),
    a::row(K::WriterVersion, "LAS X 2", 2, 2, 2),
    a::row(K::WriterVersion, "LAS X 3", 24, 19, 25),
    a::row(K::WriterVersion, "LAS X 4", 9, 9, 9),
    a::row(K::WriterVersion, "LAS-AF 1", 1, 1, 1),
];
// END GENERATED lif
