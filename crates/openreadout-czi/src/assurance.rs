//! Assurance profile (`docs/assurance.md`): the variant features of a CZI file and the feature
//! values the development corpus validates. The table between the GENERATED markers is written
//! by `cargo xtask assurance-audit --write` from `corpus/assurance/evidence.json`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static CZI: AssuranceProfile = AssuranceProfile {
    format_id: "czi",
    observe,
    validated: CZI_VALIDATED,
    confidence: CZI_CONFIDENCE,
    basis: Basis::PriorArt,
};

/// ZEN writes CZI as "ZEN (blue edition)", "ZEN 2.3 (blue edition)", ZEN black as
/// "AIMApplication"; other writers are libraries (pylibCZIrw) or test tools.
fn writer_family(sw: &str) -> String {
    let l = sw.to_ascii_lowercase();
    if l.starts_with("zen") && l.contains("blue") {
        "ZEN blue".into()
    } else if l.starts_with("aim") || (l.starts_with("zen") && l.contains("black")) {
        "ZEN black".into()
    } else if l.starts_with("zen") {
        "ZEN".into()
    } else {
        sw.to_string()
    }
}

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for c in a::extra_values(info, "compression") {
        for codec in c.split('+') {
            o.feature(K::Codec, codec, &[Scope::Pixels]);
        }
    }
    for s in a::extra_values(info, "stored_pixel_type") {
        o.feature(K::SampleLayout, format!("stored {s}"), &[Scope::Pixels]);
    }
    for im in &info.images {
        if im.mosaic.as_ref().is_some_and(|m| m.tile_count > 1) {
            o.feature(K::Layout, "mosaic", &[Scope::Pixels]);
        }
        if im.pyramid_levels > 1 {
            o.feature(K::Layout, "pyramid", &[Scope::Pixels]);
        }
        if let Some(i) = &im.instrument {
            if let Some(sw) = &i.software {
                let fam = writer_family(sw);
                o.feature(K::Writer, &fam, &[Scope::Metadata, Scope::Pixels]);
                if let Some(v) = i
                    .software_version
                    .as_deref()
                    .and_then(|v| a::version_prefix(v, 2))
                {
                    o.context(K::WriterVersion, format!("{fam} {v}"));
                }
            }
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
        }
    }
    if info.images.len() > 1 {
        o.feature(K::Layout, "multi_scene", &[Scope::Metadata]);
    }
    // Subblocks varying along H, I, R, V or B: one image per combination of coordinates.
    let extra_axes: std::collections::BTreeSet<String> = info
        .images
        .iter()
        .filter_map(|im| im.extra.get("dimension_index"))
        .filter_map(|v| v.as_object())
        .flat_map(|m| m.keys().cloned())
        .collect();
    for axis in extra_axes {
        o.feature(
            K::Layout,
            format!("extra dimension {axis}"),
            &[Scope::Pixels, Scope::Metadata],
        );
    }
    if let Some(n) = a::note_with(info, "multi-file document:") {
        o.feature(K::Layout, "multi_file", &[Scope::Pixels]);
        // "multi-file document: N following part(s) referenced, M found next to this file"
        let nums: Vec<u64> = n
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|t| t.parse().ok())
            .collect();
        if let [referenced, found, ..] = nums[..]
            && found < referenced
        {
            o.undecoded(
                "missing file parts",
                &[Scope::Pixels],
                format!("{referenced} part(s) referenced, {found} found: subblocks stored in the missing parts cannot be read"),
            );
        }
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

/// What only the dataset knows: full-resolution subblocks stored *larger* than their logical
/// extent (super-resolved PALM/SMLM renderings). The reader exposes each stored-to-logical ratio
/// as its own image at the stored size, with the logical pixel size divided by the ratio
/// (`docs/formats/czi.md`); the variant is fingerprinted so the evidence says which files
/// validate it.
///
/// Also the JPEG coding processes beyond 8-bit sequential DCT (`jpeg 12-bit`, `jpeg lossless`):
/// they share the compression id with ordinary JPEG but take other decoding paths.
pub(crate) fn internal<'a>(
    level0: impl Iterator<Item = &'a crate::DirectoryEntry>,
    jpeg_processes: &std::collections::BTreeSet<String>,
) -> Observations {
    let mut o = Observations::default();
    for p in jpeg_processes {
        o.feature(K::Codec, p, &[Scope::Pixels]);
    }
    let super_resolved = level0
        .filter(|e| {
            let over = |d: char| {
                e.dim(d)
                    .is_some_and(|x| x.size > 0 && x.stored_size > x.size)
            };
            over('X') || over('Y')
        })
        .count();
    if super_resolved > 0 {
        o.feature(
            K::Layout,
            "super-resolved rendering",
            &[Scope::Pixels, Scope::Metadata],
        );
    }
    o
}

// BEGIN GENERATED czi (cargo xtask assurance-audit --write; do not edit)
const CZI_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CZI_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "chunked", 3, 0, 3),
    a::row(K::Codec, "jpeg", 5, 0, 5),
    a::row(K::Codec, "jpeg 12-bit", 1, 0, 1),
    a::row(K::Codec, "jpeg lossless", 1, 0, 1),
    a::row(K::Codec, "jpeg_xr", 13, 6, 15),
    a::row(K::Codec, "uncompressed", 62, 18, 64),
    a::row(K::Codec, "zstd0", 1, 1, 1),
    a::row(K::Codec, "zstd1", 4, 3, 4),
    a::row(K::FormatVersion, "1.0", 88, 25, 93),
    a::row(K::Instrument, "Andor1, AxioObserver", 3, 3, 3),
    a::row(K::Instrument, "Axio Imager.Z1", 1, 1, 1),
    a::row(K::Instrument, "Axio Imager.Z2", 1, 1, 3),
    a::row(K::Instrument, "Axio Observer.Z1 / 7", 7, 4, 7),
    a::row(K::Instrument, "Axio Scan.Z1", 6, 5, 7),
    a::row(K::Instrument, "Axio Zoom.V16", 2, 2, 2),
    a::row(K::Instrument, "Axioscan 7", 6, 1, 6),
    a::row(K::Instrument, "Celldiscoverer 7", 27, 2, 27),
    a::row(K::Instrument, "LSM 510, AxioObserver", 1, 1, 1),
    a::row(K::Instrument, "LSM 700, AxioObserver", 1, 1, 1),
    a::row(K::Instrument, "LSM 710, Axio Examiner", 1, 1, 1),
    a::row(K::Instrument, "LSM 710, AxioObserver", 3, 1, 3),
    a::row(K::Instrument, "LSM 780, Axio Examiner", 1, 1, 1),
    a::row(K::Instrument, "LSM 780, AxioObserver", 3, 3, 3),
    a::row(K::Instrument, "LSM 880, AxioObserver", 2, 2, 2),
    a::row(K::Layout, "extra dimension H", 3, 3, 3),
    a::row(K::Layout, "mosaic", 29, 8, 31),
    a::row(K::Layout, "multi_file", 1, 0, 1),
    a::row(K::Layout, "multi_scene", 35, 10, 35),
    a::row(K::Layout, "pyramid", 22, 8, 23),
    a::row(K::Layout, "super-resolved rendering", 3, 1, 3),
    a::row(K::SampleLayout, "stored bgr24", 11, 5, 12),
    a::row(K::SampleLayout, "stored bgr48", 5, 1, 5),
    a::row(K::SampleLayout, "stored gray16", 56, 18, 58),
    a::row(K::SampleLayout, "stored gray8", 16, 7, 17),
    a::row(K::SampleLayout, "uint16", 56, 18, 58),
    a::row(K::SampleLayout, "uint16x3", 5, 1, 5),
    a::row(K::SampleLayout, "uint8", 16, 7, 17),
    a::row(K::SampleLayout, "uint8x3", 11, 5, 12),
    a::row(K::Writer, "ZEN", 7, 6, 7),
    a::row(K::Writer, "ZEN black", 17, 11, 17),
    a::row(K::Writer, "ZEN blue", 44, 10, 47),
    a::row(K::Writer, "pylibCZIrw", 18, 0, 19),
    a::row(K::WriterVersion, "ZEN 3.10", 3, 3, 3),
    a::row(K::WriterVersion, "ZEN 3.13", 1, 1, 1),
    a::row(K::WriterVersion, "ZEN 3.8", 2, 1, 2),
    a::row(K::WriterVersion, "ZEN 3.9", 1, 1, 1),
    a::row(K::WriterVersion, "ZEN black 11.0", 2, 1, 2),
    a::row(K::WriterVersion, "ZEN black 14.0", 7, 7, 7),
    a::row(K::WriterVersion, "ZEN black 16.0", 2, 2, 2),
    a::row(K::WriterVersion, "ZEN black 7.0", 6, 2, 6),
    a::row(K::WriterVersion, "ZEN blue 1.1", 2, 1, 5),
    a::row(K::WriterVersion, "ZEN blue 2.3", 5, 2, 5),
    a::row(K::WriterVersion, "ZEN blue 2.6", 2, 2, 2),
    a::row(K::WriterVersion, "ZEN blue 3.0", 1, 1, 1),
    a::row(K::WriterVersion, "ZEN blue 3.3", 5, 2, 5),
    a::row(K::WriterVersion, "ZEN blue 3.4", 3, 2, 3),
    a::row(K::WriterVersion, "ZEN blue 3.5", 6, 1, 6),
    a::row(K::WriterVersion, "ZEN blue 3.6", 18, 1, 18),
    a::row(K::WriterVersion, "ZEN blue 3.7", 2, 2, 2),
    a::row(K::WriterVersion, "pylibCZIrw 6.1", 18, 0, 19),
];
// END GENERATED czi
