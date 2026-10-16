//! Look-ups in the data dictionary (`dictionary_table.rs`, generated from PS3.6) and the UID
//! registry (PS3.6 Annex A) for the transfer syntaxes and storage classes this reader names.

use super::dictionary_table::{ELEMENTS, MASKED};

/// Keyword and VR of `tag`; `None` for private and unknown tags.
pub(crate) fn lookup(tag: u32) -> Option<(&'static str, &'static str)> {
    if let Ok(i) = ELEMENTS.binary_search_by_key(&tag, |e| e.0) {
        let (_, k, v) = ELEMENTS[i];
        return Some((k, v));
    }
    MASKED
        .iter()
        .find(|(value, mask, _, _)| tag & mask == *value)
        .map(|(_, _, k, v)| (*k, *v))
}

/// `(gggg,eeee)`.
pub(crate) fn tag_text(tag: u32) -> String {
    format!("({:04X},{:04X})", tag >> 16, tag & 0xFFFF)
}

/// The name an element is listed under in the vendor tree: its keyword, or `(gggg,eeee)` for
/// private and unknown tags.
pub(crate) fn name(tag: u32) -> String {
    lookup(tag).map_or_else(|| tag_text(tag), |(k, _)| k.to_string())
}

/// How the pixel data of a transfer syntax is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compression {
    /// Native (uncompressed) samples.
    Native,
    /// Uncompressed samples in fragments (1.2.840.10008.1.2.1.98).
    EncapsulatedNative,
    /// JPEG, baseline, extended or lossless (ITU-T T.81).
    Jpeg,
    /// JPEG-LS (ITU-T T.87).
    JpegLs,
    /// JPEG 2000 and High-Throughput JPEG 2000.
    Jpeg2000,
    /// JPEG XL.
    JpegXl,
    /// RLE Lossless (PS3.5 Annex G).
    Rle,
    /// Each frame deflated (Deflated Image Frame Compression, PS3.5 A.4.12).
    DeflateFrame,
    /// Video (MPEG-2/4, HEVC) and anything unknown.
    Other,
}

/// A transfer syntax: encoding of the data set and of the pixel data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Syntax {
    pub(crate) uid: &'static str,
    pub(crate) name: &'static str,
    pub(crate) explicit: bool,
    pub(crate) little: bool,
    pub(crate) deflated: bool,
    pub(crate) compression: Compression,
    pub(crate) lossy: bool,
}

const fn ts(
    uid: &'static str,
    name: &'static str,
    compression: Compression,
    lossy: bool,
) -> Syntax {
    Syntax {
        uid,
        name,
        explicit: true,
        little: true,
        deflated: false,
        compression,
        lossy,
    }
}

/// Transfer syntaxes of PS3.6 Annex A.
pub(crate) static SYNTAXES: &[Syntax] = &[
    Syntax {
        explicit: false,
        ..ts(
            "1.2.840.10008.1.2",
            "Implicit VR Little Endian",
            Compression::Native,
            false,
        )
    },
    ts(
        "1.2.840.10008.1.2.1",
        "Explicit VR Little Endian",
        Compression::Native,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.1.98",
        "Encapsulated Uncompressed Explicit VR Little Endian",
        Compression::EncapsulatedNative,
        false,
    ),
    Syntax {
        deflated: true,
        ..ts(
            "1.2.840.10008.1.2.1.99",
            "Deflated Explicit VR Little Endian",
            Compression::Native,
            false,
        )
    },
    Syntax {
        little: false,
        ..ts(
            "1.2.840.10008.1.2.2",
            "Explicit VR Big Endian",
            Compression::Native,
            false,
        )
    },
    ts(
        "1.2.840.10008.1.2.4.50",
        "JPEG Baseline (Process 1)",
        Compression::Jpeg,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.51",
        "JPEG Extended (Process 2 & 4)",
        Compression::Jpeg,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.57",
        "JPEG Lossless, Non-Hierarchical (Process 14)",
        Compression::Jpeg,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.70",
        "JPEG Lossless, Non-Hierarchical, First-Order Prediction (Process 14 [Selection Value 1])",
        Compression::Jpeg,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.80",
        "JPEG-LS Lossless Image Compression",
        Compression::JpegLs,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.81",
        "JPEG-LS Lossy (Near-Lossless) Image Compression",
        Compression::JpegLs,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.52",
        "JPEG Extended (Process 3 & 5) (Retired)",
        Compression::Jpeg,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.53",
        "JPEG Spectral Selection, Non-Hierarchical (Process 6 & 8) (Retired)",
        Compression::Jpeg,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.55",
        "JPEG Full Progression, Non-Hierarchical (Process 10 & 12) (Retired)",
        Compression::Jpeg,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.90",
        "JPEG 2000 Image Compression (Lossless Only)",
        Compression::Jpeg2000,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.91",
        "JPEG 2000 Image Compression",
        Compression::Jpeg2000,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.92",
        "JPEG 2000 Part 2 Multi-component Image Compression (Lossless Only)",
        Compression::Jpeg2000,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.93",
        "JPEG 2000 Part 2 Multi-component Image Compression",
        Compression::Jpeg2000,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.201",
        "High-Throughput JPEG 2000 Image Compression (Lossless Only)",
        Compression::Jpeg2000,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.202",
        "High-Throughput JPEG 2000 with RPCL Options Image Compression (Lossless Only)",
        Compression::Jpeg2000,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.203",
        "High-Throughput JPEG 2000 Image Compression",
        Compression::Jpeg2000,
        true,
    ),
    ts(
        "1.2.840.10008.1.2.4.110",
        "JPEG XL Lossless",
        Compression::JpegXl,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.4.111",
        "JPEG XL JPEG Recompression",
        Compression::JpegXl,
        true,
    ),
    ts("1.2.840.10008.1.2.4.112", "JPEG XL", Compression::JpegXl, true),
    ts(
        "1.2.840.10008.1.2.8.1",
        "Deflated Image Frame Compression",
        Compression::DeflateFrame,
        false,
    ),
    ts(
        "1.2.840.10008.1.2.5",
        "RLE Lossless",
        Compression::Rle,
        false,
    ),
];

/// The transfer syntax with `uid`; unknown UIDs are treated as explicit VR little endian with
/// encapsulated pixel data this reader does not decode (`Compression::Other`).
pub(crate) fn syntax(uid: &str) -> Syntax {
    SYNTAXES.iter().find(|s| s.uid == uid).copied().unwrap_or(Syntax {
        uid: "",
        name: "unknown transfer syntax",
        ..ts("", "", Compression::Other, true)
    })
}

/// Names of the storage SOP classes (PS3.6 Annex A) a lab is likely to meet.
static SOP_CLASSES: &[(&str, &str)] = &[
    ("1.2.840.10008.1.3.10", "Media Storage Directory Storage"),
    ("1.2.840.10008.5.1.4.1.1.1", "Computed Radiography Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.1.1", "Digital X-Ray Image Storage - For Presentation"),
    ("1.2.840.10008.5.1.4.1.1.1.1.1", "Digital X-Ray Image Storage - For Processing"),
    ("1.2.840.10008.5.1.4.1.1.1.2", "Digital Mammography X-Ray Image Storage - For Presentation"),
    ("1.2.840.10008.5.1.4.1.1.1.2.1", "Digital Mammography X-Ray Image Storage - For Processing"),
    ("1.2.840.10008.5.1.4.1.1.1.3", "Digital Intra-Oral X-Ray Image Storage - For Presentation"),
    ("1.2.840.10008.5.1.4.1.1.2", "CT Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.2.1", "Enhanced CT Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.2.2", "Legacy Converted Enhanced CT Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.3.1", "Ultrasound Multi-frame Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.4", "MR Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.4.1", "Enhanced MR Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.4.2", "MR Spectroscopy Storage"),
    ("1.2.840.10008.5.1.4.1.1.4.3", "Enhanced MR Color Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.4.4", "Legacy Converted Enhanced MR Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.6.1", "Ultrasound Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.6.2", "Enhanced US Volume Storage"),
    ("1.2.840.10008.5.1.4.1.1.7", "Secondary Capture Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.7.1", "Multi-frame Single Bit Secondary Capture Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.7.2", "Multi-frame Grayscale Byte Secondary Capture Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.7.3", "Multi-frame Grayscale Word Secondary Capture Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.7.4", "Multi-frame True Color Secondary Capture Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.9.1.1", "12-lead ECG Waveform Storage"),
    ("1.2.840.10008.5.1.4.1.1.11.1", "Grayscale Softcopy Presentation State Storage"),
    ("1.2.840.10008.5.1.4.1.1.12.1", "X-Ray Angiographic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.12.1.1", "Enhanced XA Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.12.2", "X-Ray Radiofluoroscopic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.13.1.1", "X-Ray 3D Angiographic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.13.1.3", "Breast Tomosynthesis Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.20", "Nuclear Medicine Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.30", "Parametric Map Storage"),
    ("1.2.840.10008.5.1.4.1.1.66", "Raw Data Storage"),
    ("1.2.840.10008.5.1.4.1.1.66.4", "Segmentation Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.1", "VL Endoscopic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.2", "VL Microscopic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.3", "VL Slide-Coordinates Microscopic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.4", "VL Photographic Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.5.1", "Ophthalmic Photography 8 Bit Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.5.4", "Ophthalmic Tomography Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.77.1.6", "VL Whole Slide Microscopy Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.88.11", "Basic Text SR Storage"),
    ("1.2.840.10008.5.1.4.1.1.88.22", "Enhanced SR Storage"),
    ("1.2.840.10008.5.1.4.1.1.88.33", "Comprehensive SR Storage"),
    ("1.2.840.10008.5.1.4.1.1.104.1", "Encapsulated PDF Storage"),
    ("1.2.840.10008.5.1.4.1.1.128", "Positron Emission Tomography Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.130", "Enhanced PET Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.128.1", "Legacy Converted Enhanced PET Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.481.1", "RT Image Storage"),
    ("1.2.840.10008.5.1.4.1.1.481.2", "RT Dose Storage"),
    ("1.2.840.10008.5.1.4.1.1.481.3", "RT Structure Set Storage"),
    ("1.2.840.10008.5.1.4.1.1.481.5", "RT Plan Storage"),
];

/// The name of a SOP class UID, when it is one of the common storage classes.
pub(crate) fn sop_class_name(uid: &str) -> Option<&'static str> {
    SOP_CLASSES.iter().find(|(u, _)| *u == uid).map(|(_, n)| *n)
}

/// The VL Whole Slide Microscopy Image Storage class.
pub(crate) const WSI_CLASS: &str = "1.2.840.10008.5.1.4.1.1.77.1.6";
/// The Media Storage Directory (DICOMDIR) class.
pub(crate) const DICOMDIR_CLASS: &str = "1.2.840.10008.1.3.10";
