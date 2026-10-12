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

pub(crate) static ISYNTAX: AssuranceProfile = AssuranceProfile {
    format_id: "philips-isyntax",
    observe: observe_isyntax,
    validated: PHILIPS_ISYNTAX_VALIDATED,
    confidence: PHILIPS_ISYNTAX_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_isyntax(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for im in &info.images {
        let Some(m) = im.extra.get("isyntax") else {
            continue;
        };
        let s = |k: &str| m.get(k).and_then(serde_json::Value::as_str);
        if let Some(v) = s("data_model").and_then(|v| a::version_prefix(v, 1)) {
            o.feature(
                K::FormatVersion,
                format!("data model {v}"),
                &[Scope::Metadata, Scope::Pixels],
            );
        }
        if let Some(c) = s("compression_method") {
            o.feature(
                K::Codec,
                format!("isyntax wavelet method {c}"),
                &[Scope::Pixels],
            );
        }
        if let Some(b) = m.get("block_size").and_then(serde_json::Value::as_array) {
            let side = |i: usize| b.get(i).and_then(serde_json::Value::as_u64).unwrap_or(0);
            o.feature(
                K::Layout,
                format!("code blocks {} x {}", side(0), side(1)),
                &[Scope::Pixels],
            );
        }
        if let Some(v) = m
            .get("software_versions")
            .and_then(|v| v.get(0))
            .and_then(serde_json::Value::as_str)
            .and_then(|v| a::version_prefix(v, 2))
        {
            o.context(K::WriterVersion, format!("scanner software {v}"));
        }
    }
    o
}

pub(crate) static VMS: AssuranceProfile = AssuranceProfile {
    format_id: "hamamatsu-vms",
    observe: observe_vms,
    validated: HAMAMATSU_VMS_VALIDATED,
    confidence: HAMAMATSU_VMS_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_vms(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for im in &info.images {
        let Some(m) = im.extra.get("vms") else {
            continue;
        };
        o.feature(K::Codec, "jpeg restart intervals", &[Scope::Pixels]);
        if let Some(g) = m.get("image_grid").and_then(serde_json::Value::as_array) {
            let n = |i: usize| g.get(i).and_then(serde_json::Value::as_u64).unwrap_or(0);
            let kind = if n(0) * n(1) > 1 {
                "grid of slide images"
            } else {
                "single slide image"
            };
            o.feature(K::Layout, kind, &[Scope::Pixels]);
        }
        match m.get("map_downsample").and_then(serde_json::Value::as_u64) {
            Some(d) => o.feature(
                K::Layout,
                format!("map at 1/{d}"),
                &[Scope::Metadata, Scope::Pixels],
            ),
            None => o.feature(K::Layout, "no usable map", &[Scope::Metadata]),
        }
        if m.get("optimisation_file")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            o.feature(K::Layout, "optimisation file", &[Scope::Pixels]);
        }
    }
    o
}

// BEGIN GENERATED hamamatsu-vms (cargo xtask assurance-audit --write; do not edit)
const HAMAMATSU_VMS_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const HAMAMATSU_VMS_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "jpeg restart intervals", 1, 1, 1),
    a::row(K::Layout, "grid of slide images", 1, 1, 1),
    a::row(K::Layout, "map at 1/8", 1, 1, 1),
    a::row(K::Layout, "optimisation file", 1, 1, 1),
    a::row(K::SampleLayout, "uint8x3", 1, 1, 1),
];
// END GENERATED hamamatsu-vms

// BEGIN GENERATED philips-isyntax (cargo xtask assurance-audit --write; do not edit)
const PHILIPS_ISYNTAX_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const PHILIPS_ISYNTAX_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "isyntax wavelet method 16", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 1),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 1),
    a::row(K::FormatVersion, "5.0", 1, 1, 1),
    a::row(K::FormatVersion, "data model 5", 1, 1, 1),
    a::row(K::Layout, "code blocks 128 x 128", 1, 1, 1),
    a::row(K::SampleLayout, "uint8x3", 1, 1, 1),
    a::row(K::WriterVersion, "scanner software 1.8", 1, 1, 1),
];
// END GENERATED philips-isyntax
