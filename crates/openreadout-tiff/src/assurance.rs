//! Assurance profile (`docs/assurance.md`): the variant features of a TIFF-family file (sub-format,
//! writer, codec with its colour space, sample arrangement, layout) and the feature values the
//! development corpus validates. The table between the GENERATED markers is written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

use crate::decode::{PageLayout, compression_name};

pub(crate) static TIFF: AssuranceProfile = AssuranceProfile {
    format_id: "tiff",
    observe,
    validated: TIFF_VALIDATED,
    confidence: TIFF_CONFIDENCE,
    basis: Basis::OpenSpec,
};

/// A writer's name without its version: `OME Bio-Formats 5.2.2` → `Bio-Formats`,
/// `tifffile.py 2020.9.3` → `tifffile`, `Aperio Image Library v11.2.1` → `Aperio Image Library`,
/// `MetaMorph 6.2.3.733` → `MetaMorph`.
fn writer_name(s: &str) -> String {
    let s = s.trim().trim_start_matches("OME ").trim();
    let mut words: Vec<&str> = Vec::new();
    for w in s.split_whitespace() {
        let t = w.trim_start_matches(['v', 'V']);
        if t.starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        words.push(w);
    }
    let name = if words.is_empty() {
        s
    } else {
        &words.join(" ")
    };
    name.trim_end_matches(".py").to_string()
}

fn writer(o: &mut Observations, name: &str, full: &str) {
    let n = writer_name(name);
    if n.is_empty() {
        return;
    }
    // Writers differ in how they lay out and encode pixels (a Bio-Formats JPEG OME-TIFF is not
    // an Aperio one), so the writer is structural for pixels as well as metadata.
    o.feature(K::Writer, &n, &[Scope::Metadata, Scope::Pixels]);
    if let Some(v) = a::version_prefix(full.trim_start_matches(&n).trim(), 1)
        .or_else(|| a::version_prefix(full, 1))
    {
        o.context(K::WriterVersion, format!("{n} {v}"));
    }
}

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        for part in v.split("; ") {
            if part.starts_with("OME-XML") {
                o.feature(K::FormatVersion, part, &[Scope::Metadata]);
            } else {
                o.feature(K::FormatVersion, part, &[Scope::Metadata, Scope::Pixels]);
            }
        }
    }
    for im in &info.images {
        let layout = if im.samples_per_pixel > 1 {
            format!("{}x{}", im.pixel_type.ome_name(), im.samples_per_pixel)
        } else {
            im.pixel_type.ome_name().to_string()
        };
        o.feature(K::SampleLayout, layout, &[Scope::Pixels]);
        if im.pyramid_levels > 1 {
            o.feature(K::Layout, "pyramid", &[Scope::Pixels]);
        }
        if im.extra.contains_key("files") {
            o.feature(K::Layout, "multi_file", &[Scope::Pixels, Scope::Metadata]);
        }
        if let Some(c) = a::extra_str(&im.extra, "ome_creator") {
            writer(&mut o, c, c);
        }
        if let Some(v) = a::extra_str(&im.extra, "imagej_version") {
            writer(&mut o, "ImageJ", &format!("ImageJ {v}"));
        }
        if let Some(i) = &im.instrument {
            if let Some(sw) = &i.software {
                let full = match &i.software_version {
                    Some(v) => format!("{sw} {v}"),
                    None => sw.clone(),
                };
                writer(&mut o, sw, &full);
            }
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
        }
    }
    if let Some(n) = a::note_with(info, "TIFF sub-format: ") {
        let flavor = n.trim_start_matches("TIFF sub-format: ");
        o.feature(K::Dialect, flavor, &[Scope::Metadata, Scope::Pixels]);
    }
    if a::note_with(info, "Molecular Dynamics GEL (").is_some() {
        // the conversion of square-root (or scaled linear) samples to counts
        let encoding = info
            .images
            .first()
            .and_then(|i| i.extra.get("md_gel"))
            .and_then(|g| g.get("encoding"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        o.feature(
            K::SampleLayout,
            format!("Molecular Dynamics GEL, {encoding} data"),
            &[Scope::Pixels],
        );
    }
    if let Some(n) = a::note_with(
        info,
        "tiles are returned on their stored grid, not stitched",
    ) {
        o.undecoded(
            "tile overlaps (Ventana BIF)",
            &[Scope::Pixels],
            n.to_string(),
        );
    }
    for n in info
        .notes
        .iter()
        .filter(|n| n.contains("falls back to") || n.contains("fell back to"))
    {
        o.undecoded("sub-format metadata", &[Scope::Metadata], n.clone());
    }
    o
}

/// Lossy codecs whose decoded colours depend on the photometric interpretation (a JPEG stored
/// as YCbCr is not decoded like one stored as RGB).
fn colour_sensitive(code: u16) -> bool {
    matches!(
        code,
        6 | 7 | 33003 | 33004 | 33005 | 34712 | 50001 | 50002 | 52546
    )
}

fn photometric_name(p: u16) -> String {
    match p {
        0 => "miniswhite".into(),
        1 => "minisblack".into(),
        2 => "rgb".into(),
        3 => "palette".into(),
        4 => "mask".into(),
        5 => "separated".into(),
        6 => "ycbcr".into(),
        8 => "cielab".into(),
        32844 | 32845 => "logluv".into(),
        n => format!("photometric {n}"),
    }
}

/// What only the dataset knows: how the pages of each image are encoded.
pub(crate) fn internal<'a>(layouts: impl Iterator<Item = &'a PageLayout>) -> Observations {
    let mut o = Observations::default();
    for l in layouts {
        let codec = compression_name(l.compression);
        let value = if let Some(e) = l.eer {
            // EER: the bit sizes of the event code are the variant (BitsPerSample is unused)
            format!("eer {}+{}+{}", e.skip_bits, e.horz_bits, e.vert_bits)
        } else if colour_sensitive(l.compression) {
            format!("{codec} ({})", photometric_name(l.photometric))
        } else {
            codec
        };
        o.feature(K::Codec, value, &[Scope::Pixels]);
        if l.samples_per_pixel > 1 {
            o.feature(
                K::SampleLayout,
                if l.planar == 2 {
                    "planar samples"
                } else {
                    "interleaved samples"
                },
                &[Scope::Pixels],
            );
        }
        if l.predictor > 1 {
            o.feature(
                K::Codec,
                format!("predictor {}", l.predictor),
                &[Scope::Pixels],
            );
        }
        if l.eer.is_none() && !matches!(l.bits_per_sample, 8 | 16 | 32 | 64) {
            o.feature(
                K::SampleLayout,
                format!("{} bits per sample", l.bits_per_sample),
                &[Scope::Pixels],
            );
        }
        // stored samples that are converted on the way out (half floats, complex integers)
        match (l.bits_per_sample, l.sample_format) {
            (16, 3) => o.feature(K::SampleLayout, "half-float samples", &[Scope::Pixels]),
            (_, 5) => o.feature(K::SampleLayout, "complex-integer samples", &[Scope::Pixels]),
            (_, 6) => o.feature(K::SampleLayout, "complex-float samples", &[Scope::Pixels]),
            _ => {}
        }
        if l.fill_order == 2 {
            o.feature(K::SampleLayout, "fill order 2", &[Scope::Pixels]);
        }
        o.feature(
            K::Layout,
            if l.tiled { "tiles" } else { "strips" },
            &[Scope::Pixels],
        );
    }
    o
}

// BEGIN GENERATED tiff (cargo xtask assurance-audit --write; do not edit)
const TIFF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const TIFF_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "deflate", 6, 3, 6),
    a::row(K::Codec, "eer 7+2+2", 3, 3, 3),
    a::row(K::Codec, "jpeg (minisblack)", 4, 2, 4),
    a::row(K::Codec, "jpeg (rgb)", 1, 1, 3),
    a::row(K::Codec, "jpeg (ycbcr)", 3, 2, 7),
    a::row(K::Codec, "jpeg-2000 (rgb)", 1, 1, 2),
    a::row(K::Codec, "jpeg-xl (rgb)", 1, 1, 1),
    a::row(K::Codec, "lzw", 5, 3, 5),
    a::row(K::Codec, "none", 75, 15, 75),
    a::row(K::Codec, "old-jpeg (ycbcr)", 0, 0, 1),
    a::row(K::Codec, "packbits", 4, 1, 4),
    a::row(K::Codec, "predictor 2", 2, 2, 2),
    a::row(K::Codec, "webp (rgb)", 2, 1, 2),
    a::row(K::Codec, "zstd", 1, 1, 1),
    a::row(K::Dialect, "aperio-svs", 4, 1, 4),
    a::row(K::Dialect, "hamamatsu-ndpi", 1, 1, 1),
    a::row(K::Dialect, "imagej", 4, 3, 4),
    a::row(K::Dialect, "leica-scn", 2, 1, 2),
    a::row(K::Dialect, "metamorph-nd", 4, 3, 4),
    a::row(K::Dialect, "metamorph-stk", 2, 2, 2),
    a::row(K::Dialect, "metaseries", 1, 1, 1),
    a::row(K::Dialect, "nis-elements", 1, 1, 1),
    a::row(K::Dialect, "ome-companion", 1, 1, 1),
    a::row(K::Dialect, "ome-tiff", 40, 5, 40),
    a::row(K::Dialect, "perkinelmer-qptiff", 2, 1, 2),
    a::row(K::Dialect, "philips-tiff", 2, 1, 2),
    a::row(K::Dialect, "plain", 45, 6, 46),
    a::row(K::Dialect, "thermo-eer", 3, 3, 3),
    a::row(K::Dialect, "ventana-bif", 1, 1, 1),
    a::row(K::Dialect, "zeiss-lsm", 3, 2, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 18),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 8),
    a::row(K::FormatVersion, "6.0", 96, 19, 97),
    a::row(K::FormatVersion, "6.0+BigTIFF", 15, 7, 15),
    a::row(K::FormatVersion, "MetaMorph ND 1.0", 1, 1, 1),
    a::row(K::FormatVersion, "MetaMorph ND 2.0", 3, 2, 3),
    a::row(K::FormatVersion, "OME-XML 2015-01", 2, 1, 2),
    a::row(K::FormatVersion, "OME-XML 2016-06", 39, 4, 39),
    a::row(K::Instrument, "Amersham TYPHOON", 1, 1, 1),
    a::row(K::Instrument, "Amersham Typhoon", 1, 1, 1),
    a::row(K::Instrument, "Eclipse TE300", 6, 1, 6),
    a::row(K::Instrument, "Leica SCN400", 1, 1, 1),
    a::row(K::Instrument, "Leica SCN400F", 1, 1, 1),
    a::row(K::Instrument, "NanoZoomer", 1, 1, 1),
    a::row(K::Instrument, "ScanScope CPAPERIOCS", 3, 1, 3),
    a::row(K::Instrument, "ScanScope SS1283", 1, 1, 1),
    a::row(K::Instrument, "Typhoon FLA 9500", 1, 1, 1),
    a::row(K::Instrument, "VENTANA DP 200", 1, 1, 1),
    a::row(K::Layout, "multi_file", 17, 6, 17),
    a::row(K::Layout, "pyramid", 9, 5, 16),
    a::row(K::Layout, "strips", 92, 19, 93),
    a::row(K::Layout, "tiles", 15, 4, 22),
    a::row(K::SampleLayout, "1 bits per sample", 6, 1, 6),
    a::row(K::SampleLayout, "10 bits per sample", 1, 1, 1),
    a::row(K::SampleLayout, "12 bits per sample", 1, 1, 1),
    a::row(K::SampleLayout, "128 bits per sample", 1, 1, 1),
    a::row(K::SampleLayout, "24 bits per sample", 1, 1, 1),
    a::row(K::SampleLayout, "Molecular Dynamics GEL, square root data", 3, 3, 3),
    a::row(K::SampleLayout, "complex", 4, 1, 4),
    a::row(K::SampleLayout, "complex-float samples", 3, 1, 3),
    a::row(K::SampleLayout, "complex-integer samples", 4, 1, 4),
    a::row(K::SampleLayout, "double", 2, 2, 2),
    a::row(K::SampleLayout, "double-complex", 3, 1, 3),
    a::row(K::SampleLayout, "float", 8, 6, 8),
    a::row(K::SampleLayout, "half-float samples", 1, 1, 1),
    a::row(K::SampleLayout, "int64", 1, 1, 1),
    a::row(K::SampleLayout, "int8", 13, 1, 13),
    a::row(K::SampleLayout, "interleaved samples", 16, 4, 24),
    a::row(K::SampleLayout, "planar samples", 6, 3, 6),
    a::row(K::SampleLayout, "uint16", 28, 11, 28),
    a::row(K::SampleLayout, "uint64", 1, 1, 1),
    a::row(K::SampleLayout, "uint8", 36, 8, 36),
    a::row(K::SampleLayout, "uint8x2", 2, 1, 2),
    a::row(K::SampleLayout, "uint8x3", 13, 4, 21),
    a::row(K::SampleLayout, "uint8x4", 1, 1, 1),
    a::row(K::Writer, "Amersham TYPHOON Scanner Control Software", 1, 1, 1),
    a::row(K::Writer, "Amersham Typhoon Scanner Control Software", 1, 1, 1),
    a::row(K::Writer, "Aperio Image Library", 4, 1, 4),
    a::row(K::Writer, "Bio-Formats", 32, 4, 32),
    a::row(K::Writer, "ImageJ", 4, 3, 4),
    a::row(K::Writer, "Jenga", 1, 1, 1),
    a::row(K::Writer, "MetaMorph", 4, 2, 4),
    a::row(K::Writer, "Micro-Manager", 3, 2, 3),
    a::row(K::Writer, "NDP.scan", 1, 1, 1),
    a::row(K::Writer, "NIS-Elements", 1, 1, 1),
    a::row(K::Writer, "Philips DP", 2, 1, 2),
    a::row(K::Writer, "ScanOutputManager", 1, 1, 1),
    a::row(K::Writer, "Typhoon FLA", 1, 1, 1),
    a::row(K::Writer, "VectraPolaris", 1, 1, 1),
    a::row(K::Writer, "VisiView", 3, 2, 3),
    a::row(K::Writer, "tifffile", 1, 1, 1),
    a::row(K::WriterVersion, "Amersham TYPHOON Scanner Control Software 3", 1, 1, 1),
    a::row(K::WriterVersion, "Amersham Typhoon Scanner Control Software 2", 1, 1, 1),
    a::row(K::WriterVersion, "Aperio Image Library 10", 2, 1, 2),
    a::row(K::WriterVersion, "Aperio Image Library 11", 2, 1, 2),
    a::row(K::WriterVersion, "Bio-Formats 5", 24, 3, 24),
    a::row(K::WriterVersion, "Bio-Formats 6", 7, 2, 7),
    a::row(K::WriterVersion, "Bio-Formats 7", 1, 1, 1),
    a::row(K::WriterVersion, "ImageJ 1", 4, 3, 4),
    a::row(K::WriterVersion, "MetaMorph 7", 4, 2, 4),
    a::row(K::WriterVersion, "Philips DP 1", 2, 1, 2),
    a::row(K::WriterVersion, "ScanOutputManager 1", 1, 1, 1),
    a::row(K::WriterVersion, "Typhoon FLA 9500", 1, 1, 1),
    a::row(K::WriterVersion, "VectraPolaris 1", 1, 1, 1),
    a::row(K::WriterVersion, "VisiView 3", 2, 1, 2),
    a::row(K::WriterVersion, "VisiView 4", 1, 1, 1),
    a::row(K::WriterVersion, "tifffile 2020", 1, 1, 1),
];
// END GENERATED tiff
