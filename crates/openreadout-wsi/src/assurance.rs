//! Assurance profile (`docs/assurance.md`) of MIRAX slides: the variant features that change how
//! the pyramid is placed and decoded, and the feature values the development corpus validates.
//! The table between the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static MIRAX: AssuranceProfile = AssuranceProfile {
    format_id: "mirax",
    observe,
    validated: MIRAX_VALIDATED,
    confidence: MIRAX_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for im in &info.images {
        if let Some(c) = a::extra_str(&im.extra, "codec") {
            o.feature(K::Codec, c, &[Scope::Pixels]);
        }
        let Some(m) = im.extra.get("mirax") else {
            continue;
        };
        let s = |k: &str| m.get(k).and_then(serde_json::Value::as_str);
        match s("camera_positions") {
            Some(src) => o.feature(
                K::Layout,
                format!("camera positions: {src}"),
                &[Scope::Metadata, Scope::Pixels],
            ),
            None => o.undecoded(
                "camera positions (none: exported slide)",
                &[Scope::Pixels],
                "no camera position table: pixels are refused",
            ),
        }
        if let Some(levels) = m.get("levels").and_then(serde_json::Value::as_array)
            && let Some(l0) = levels.first()
        {
            let step = l0
                .get("grid_step")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);
            let div = m
                .get("camera_divisions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);
            // Saved (downsampled) slides start at a coarser grid step; whether level 0 holds
            // several camera positions per image changes placement.
            let kind = match step.cmp(&div) {
                std::cmp::Ordering::Less => "level 0 below one camera photo per image",
                std::cmp::Ordering::Equal => "level 0 one camera photo per image",
                std::cmp::Ordering::Greater => "level 0 several camera photos per image",
            };
            o.feature(K::Layout, kind, &[Scope::Metadata, Scope::Pixels]);
            if step > 1 {
                o.feature(
                    K::Layout,
                    "saved at reduced resolution",
                    &[Scope::Metadata, Scope::Pixels],
                );
            }
            for lv in levels {
                if let Some(f) = lv.get("image_format").and_then(serde_json::Value::as_str) {
                    let f = f.to_ascii_lowercase();
                    let f = if f.starts_with("bmp") {
                        "bmp".to_string()
                    } else {
                        f
                    };
                    o.feature(K::Codec, f, &[Scope::Pixels]);
                }
            }
        }
        if m.get("filters").is_some() {
            o.feature(
                K::Acquisition,
                "fluorescence filters as colour components",
                &[Scope::Pixels],
            );
        } else {
            o.feature(K::Acquisition, "brightfield", &[Scope::Pixels]);
        }
        if let Some(v) =
            s("scanner_software_version").and_then(|v| a::version_prefix(&v.replace(',', "."), 2))
        {
            o.context(K::WriterVersion, format!("scanner software {v}"));
        }
    }
    if a::has_note(info, "is not grey") {
        o.assumed(
            "images[].extra.mirax.levels[].fill_color_bgr",
            "a non-grey fill colour is read as 0xBBGGRR; no validated file settles the component order",
        );
    }
    o
}

// BEGIN GENERATED mirax (cargo xtask assurance-audit --write; do not edit)
const MIRAX_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const MIRAX_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "brightfield", 5, 1, 5),
    a::row(K::Acquisition, "fluorescence filters as colour components", 2, 1, 2),
    a::row(K::Codec, "bmp", 1, 1, 1),
    a::row(K::Codec, "jpeg", 5, 1, 5),
    a::row(K::Codec, "png", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 7),
    a::row(K::FormatVersion, "1.9", 3, 1, 3),
    a::row(K::FormatVersion, "2", 2, 1, 2),
    a::row(K::FormatVersion, "2.2", 2, 1, 2),
    a::row(K::Layout, "camera positions: compressed position table", 2, 1, 2),
    a::row(K::Layout, "camera positions: position buffer", 5, 1, 5),
    a::row(K::Layout, "level 0 below one camera photo per image", 6, 1, 6),
    a::row(K::Layout, "level 0 several camera photos per image", 1, 1, 1),
    a::row(K::Layout, "saved at reduced resolution", 2, 1, 2),
    a::row(K::SampleLayout, "uint8", 2, 1, 2),
    a::row(K::SampleLayout, "uint8x3", 5, 1, 5),
    a::row(K::WriterVersion, "scanner software 1.12", 4, 1, 4),
    a::row(K::WriterVersion, "scanner software 1.15", 2, 1, 2),
    a::row(K::WriterVersion, "scanner software 2.0", 1, 1, 1),
];
// END GENERATED mirax
