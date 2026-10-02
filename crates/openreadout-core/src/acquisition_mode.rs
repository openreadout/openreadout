//! One readable vocabulary for `ChannelInfo::acquisition_mode`, and its mapping to and from the
//! OME 2016-06 `Channel/@AcquisitionMode` and `Channel/@ContrastMethod` enumerations (open
//! standard, <https://www.openmicroscopy.org/Schemas/OME/2016-06>).
//!
//! A mode is written as `<technique>[ <contrast>]`: `Laser Scanning Confocal`,
//! `Widefield Fluorescence`, `Brightfield`, `Phase Contrast`. Readers whose files store OME
//! enumeration tokens (CZI, OME-TIFF, OME-Zarr) turn them into these labels with [`label`] or
//! [`from_ome`]; the OME-XML exporter turns labels back with [`to_ome`]. Labels a vendor
//! writes in its own words (`Brightfield (RGB)`) are kept as written. The table is listed in
//! `book/src/guides/metadata.md` (§ Acquisition mode).

/// OME `AcquisitionMode` tokens and their labels. `TIRF` precedes `TotalInternalReflection` so
/// that the label maps back to the shorter token.
const MODES: &[(&str, &str)] = &[
    ("WideField", "Widefield"),
    ("LaserScanningConfocalMicroscopy", "Laser Scanning Confocal"),
    ("SpinningDiskConfocal", "Spinning Disk Confocal"),
    ("SlitScanConfocal", "Slit Scan Confocal"),
    ("SweptFieldConfocal", "Swept Field Confocal"),
    ("MultiPhotonMicroscopy", "Multiphoton"),
    ("StructuredIllumination", "Structured Illumination"),
    ("SingleMoleculeImaging", "Single Molecule Imaging"),
    ("TIRF", "TIRF"),
    ("TotalInternalReflection", "TIRF"),
    ("FluorescenceLifetime", "Fluorescence Lifetime"),
    ("SpectralImaging", "Spectral Imaging"),
    (
        "FluorescenceCorrelationSpectroscopy",
        "Fluorescence Correlation Spectroscopy",
    ),
    (
        "NearFieldScanningOpticalMicroscopy",
        "Near-Field Scanning Optical Microscopy",
    ),
    (
        "SecondHarmonicGenerationImaging",
        "Second Harmonic Generation",
    ),
    ("PALM", "PALM"),
    ("STORM", "STORM"),
    ("STED", "STED"),
    ("FSM", "FSM"),
    ("LCM", "LCM"),
    ("SPIM", "SPIM"),
    ("BrightField", "Brightfield"),
    ("Other", "Other"),
];

/// OME `ContrastMethod` tokens and their labels.
const CONTRASTS: &[(&str, &str)] = &[
    ("Fluorescence", "Fluorescence"),
    ("Brightfield", "Brightfield"),
    ("Phase", "Phase Contrast"),
    ("DIC", "DIC"),
    ("HoffmanModulation", "Hoffman Modulation Contrast"),
    ("ObliqueIllumination", "Oblique Illumination"),
    ("PolarizedLight", "Polarized Light"),
    ("Darkfield", "Darkfield"),
    ("Other", "Other"),
];

fn lookup(table: &'static [(&'static str, &'static str)], token: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(t, _)| t.eq_ignore_ascii_case(token))
        .map(|(_, l)| *l)
}

/// The readable label of a mode as a reader found it: an OME `AcquisitionMode` or
/// `ContrastMethod` token becomes its label (`WideField` → `Widefield`,
/// `LaserScanningConfocalMicroscopy` → `Laser Scanning Confocal`, `Phase` → `Phase Contrast`);
/// anything else is returned trimmed. `None` for blank text.
pub fn label(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    Some(
        lookup(MODES, t)
            .or_else(|| lookup(CONTRASTS, t))
            .map_or_else(|| t.to_string(), str::to_string),
    )
}

/// The label for an OME channel's `AcquisitionMode` and `ContrastMethod` (either may be
/// absent). Fluorescence contrast is appended to the technique (`Widefield Fluorescence`);
/// other contrasts replace a widefield or brightfield technique (`Phase Contrast`).
pub fn from_ome(mode: Option<&str>, contrast: Option<&str>) -> Option<String> {
    let m = mode.and_then(label);
    let k = contrast.and_then(label).filter(|k| k != "Other");
    match (m, k) {
        (m, None) => m,
        (None, k) => k,
        (Some(m), Some(k)) => Some(
            if m == k || (m.to_ascii_lowercase().contains("fluorescence") && k == "Fluorescence") {
                m
            } else if k == "Fluorescence" {
                format!("{m} {k}")
            } else if m == "Widefield" || m == "Brightfield" || m == "Other" {
                k
            } else {
                format!("{m} {k}")
            },
        ),
    }
}

fn has_word(l: &str, w: &str) -> bool {
    l.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|x| x == w)
}

/// OME `AcquisitionMode` and `ContrastMethod` tokens for a label (either may be `None`). For
/// the labels of this module, and for vendor labels built the same way,
/// `from_ome(to_ome(l))` gives `l` back; [`round_trips`] tells whether it does.
pub fn to_ome(label_text: &str) -> (Option<&'static str>, Option<&'static str>) {
    let Some(l) = label(label_text) else {
        return (None, None);
    };
    let l = l.to_ascii_lowercase();
    let has = |s: &str| l.contains(s);
    let mode = if has("spinning") {
        Some("SpinningDiskConfocal")
    } else if has("swept field") || has("sweptfield") {
        Some("SweptFieldConfocal")
    } else if has("slit scan") {
        Some("SlitScanConfocal")
    } else if has("confocal") {
        Some("LaserScanningConfocalMicroscopy")
    } else if has("multiphoton") || has("multi-photon") || has("two-photon") || has("2-photon") {
        Some("MultiPhotonMicroscopy")
    } else if has_word(&l, "tirf") {
        Some("TIRF")
    } else if has("total internal reflection") {
        Some("TotalInternalReflection")
    } else if has("fluorescence lifetime") || has_word(&l, "flim") {
        Some("FluorescenceLifetime")
    } else if has("fluorescence correlation") {
        Some("FluorescenceCorrelationSpectroscopy")
    } else if has("spectral imaging") {
        Some("SpectralImaging")
    } else if has("structured illumination") {
        Some("StructuredIllumination")
    } else if has("single molecule") {
        Some("SingleMoleculeImaging")
    } else if has("near-field") || has("near field") {
        Some("NearFieldScanningOpticalMicroscopy")
    } else if has("second harmonic") {
        Some("SecondHarmonicGenerationImaging")
    } else if has("light sheet") || has("lightsheet") || has_word(&l, "spim") {
        Some("SPIM")
    } else if let Some(t) = ["palm", "storm", "sted", "fsm", "lcm"]
        .iter()
        .find(|w| has_word(&l, w))
    {
        MODES
            .iter()
            .find(|(tok, _)| tok.eq_ignore_ascii_case(t))
            .map(|(tok, _)| *tok)
    } else if has("widefield") || has("wide field") || has("wide-field") {
        Some("WideField")
    } else if has("brightfield") || has("bright field") {
        Some("BrightField")
    } else if l == "other" {
        Some("Other")
    } else {
        None
    };
    let contrast = if l.ends_with("fluorescence") {
        Some("Fluorescence")
    } else if has("phase") {
        Some("Phase")
    } else if has_word(&l, "dic") || has("differential interference") {
        Some("DIC")
    } else if has("hoffman") {
        Some("HoffmanModulation")
    } else if has("oblique") {
        Some("ObliqueIllumination")
    } else if has("polariz") || has("polaris") {
        Some("PolarizedLight")
    } else if has("darkfield") || has("dark field") {
        Some("Darkfield")
    } else if has("brightfield") || has("bright field") {
        Some("Brightfield")
    } else {
        None
    };
    (mode, contrast)
}

/// True when the OME tokens of `label_text` read back as the same label, i.e. the OME
/// enumerations alone carry it through an export.
pub fn round_trips(label_text: &str) -> bool {
    let (m, k) = to_ome(label_text);
    from_ome(m, k).as_deref() == Some(label_text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_become_labels() {
        assert_eq!(label("WideField").as_deref(), Some("Widefield"));
        assert_eq!(
            label("LaserScanningConfocalMicroscopy").as_deref(),
            Some("Laser Scanning Confocal")
        );
        assert_eq!(label(" Phase ").as_deref(), Some("Phase Contrast"));
        assert_eq!(
            label("Brightfield (RGB)").as_deref(),
            Some("Brightfield (RGB)")
        );
        assert_eq!(label("  "), None);
    }

    #[test]
    fn composition() {
        assert_eq!(
            from_ome(
                Some("LaserScanningConfocalMicroscopy"),
                Some("Fluorescence")
            )
            .as_deref(),
            Some("Laser Scanning Confocal Fluorescence")
        );
        assert_eq!(
            from_ome(Some("WideField"), Some("Phase")).as_deref(),
            Some("Phase Contrast")
        );
        assert_eq!(
            from_ome(Some("BrightField"), Some("Brightfield")).as_deref(),
            Some("Brightfield")
        );
        assert_eq!(from_ome(None, Some("Other")), None);
        assert_eq!(
            from_ome(Some("FluorescenceLifetime"), Some("Fluorescence")).as_deref(),
            Some("Fluorescence Lifetime")
        );
    }

    #[test]
    fn every_reader_label_round_trips() {
        // Labels the readers emit (LIF, ND2, CZI, Opera Harmony) and every table label.
        let mut labels: Vec<&str> = vec![
            "Laser Scanning Confocal",
            "Widefield",
            "Brightfield",
            "Phase Contrast",
            "DIC",
            "Fluorescence",
            "Widefield Fluorescence",
            "Spinning Disk Confocal Fluorescence",
            "Multiphoton Fluorescence",
            "Swept Field Confocal Fluorescence",
            "Laser Scanning Confocal Fluorescence",
            "TIRF Fluorescence",
        ];
        labels.extend(MODES.iter().map(|(_, l)| *l));
        labels.extend(CONTRASTS.iter().map(|(_, l)| *l).filter(|l| *l != "Other"));
        for l in labels {
            assert!(round_trips(l), "{l}: {:?} → {:?}", to_ome(l), {
                let (m, k) = to_ome(l);
                from_ome(m, k)
            });
        }
        assert!(!round_trips("Brightfield (RGB)"));
        assert_eq!(
            to_ome("Brightfield (RGB)"),
            (Some("BrightField"), Some("Brightfield"))
        );
    }
}
