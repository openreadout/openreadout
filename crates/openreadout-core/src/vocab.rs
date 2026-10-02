//! Controlled vocabulary: the ontology terms and UCUM unit codes the experiment model emits.
//!
//! A small curated table, not the ontologies themselves: only term ids and labels are stored
//! (no definitions, synonyms or hierarchy). Sources, licences and the release each label was
//! checked against are listed in `book/src/guides/metadata.md` and `NOTICE`:
//!
//! | prefix | ontology | licence |
//! | --- | --- | --- |
//! | `MS:` | PSI-MS controlled vocabulary (HUPO-PSI) | CC BY 3.0 (OBO Foundry registry) |
//! | `CHMO:` | Chemical Methods Ontology (RSC) | CC BY 4.0 |
//! | `FBbi:` | Biological Imaging Methods Ontology | CC BY 4.0 |
//! | `OBI:` | Ontology for Biomedical Investigations | CC BY 4.0 |
//!
//! Units are written for people (`µm`) and carry their UCUM code (`um`), the Unified Code for
//! Units of Measure (Regenstrief Institute; free to use under its terms of use).
//!
//! Every term the model emits must be in [`TERMS`] and every unit in [`UNITS`]; the unit tests
//! and the corpus conformance walk check both.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An ontology term: its id (CURIE, e.g. `CHMO:0000524`) and label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Term {
    /// Compact id, `PREFIX:local` (`MS:1000130`, `CHMO:0000591`, `FBbi:00000246`, `OBI:0000916`).
    pub id: String,
    /// The term's label in its ontology (`positive scan`).
    pub label: String,
}

/// Curated terms: `(id, label)`. Labels are the ontologies' own (British spelling in CHMO).
pub const TERMS: &[(&str, &str)] = &[
    // ---- PSI-MS (mass spectrometry)
    ("MS:1000129", "negative scan"),
    ("MS:1000130", "positive scan"),
    ("MS:1000579", "MS1 spectrum"),
    ("MS:1000580", "MSn spectrum"),
    ("MS:1000073", "electrospray ionization"),
    ("MS:1000398", "nanoelectrospray"),
    ("MS:1000075", "matrix-assisted laser desorption ionization"),
    ("MS:1000070", "atmospheric pressure chemical ionization"),
    ("MS:1000389", "electron ionization"),
    ("MS:1000484", "orbitrap"),
    ("MS:1000079", "fourier transform ion cyclotron resonance"),
    ("MS:1000264", "ion trap"),
    ("MS:1000084", "time-of-flight"),
    ("MS:1000081", "quadrupole"),
    ("MS:1003221", "data-dependent acquisition"),
    ("MS:1003215", "data-independent acquisition"),
    ("MS:1000235", "total ion current chromatogram"),
    ("MS:1000483", "Thermo Fisher Scientific instrument model"),
    ("MS:1000122", "Bruker Daltonics instrument model"),
    ("MS:1000126", "Waters instrument model"),
    ("MS:1000490", "Agilent instrument model"),
    // ---- CHMO (chemical methods: MS, chromatography, spectroscopy, EM)
    ("CHMO:0000470", "mass spectrometry"),
    ("CHMO:0000575", "tandem mass spectrometry"),
    ("CHMO:0000524", "liquid chromatography-mass spectrometry"),
    (
        "CHMO:0000701",
        "liquid chromatography-tandem mass spectrometry",
    ),
    ("CHMO:0000497", "gas chromatography-mass spectrometry"),
    ("CHMO:0000053", "imaging mass spectrometry"),
    ("CHMO:0001000", "chromatography"),
    ("CHMO:0001002", "gas chromatography"),
    ("CHMO:0001004", "liquid chromatography"),
    // protein purification (ÄKTA / UNICORN methods), checked on EBI OLS 2026-09-25
    ("CHMO:0001013", "size-exclusion chromatography"),
    ("CHMO:0001006", "affinity chromatography"),
    ("CHMO:0001014", "ion-exchange chromatography"),
    // gel and blot imaging (Bio-Rad Image Lab), checked on EBI OLS 2026-09-26
    ("CHMO:0001021", "gel electrophoresis"),
    ("OBI:0000854", "western blot assay"),
    // calorimetry (MicroCal ITC), checked on EBI OLS 2026-09-26
    ("CHMO:0000683", "isothermal titration calorimetry"),
    ("OBI:0000930", "calorimeter"),
    ("CHMO:0000624", "surface plasmon resonance spectroscopy"),
    ("OBI:0001136", "surface plasmon resonance instrument"),
    ("CHMO:0001719", "flame ionisation detection"),
    ("CHMO:0001731", "thermal conductivity detection"),
    ("CHMO:0001728", "photodiode array detection"),
    ("CHMO:0001730", "refractive index detection"),
    ("CHMO:0000060", "fluorescence detection"),
    ("CHMO:0000057", "luminescence detection"),
    ("CHMO:0000292", "ultraviolet-visible spectrophotometry"),
    ("CHMO:0000630", "infrared absorption spectroscopy"),
    ("CHMO:0000656", "Raman spectroscopy"),
    ("CHMO:0000591", "nuclear magnetic resonance spectroscopy"),
    ("CHMO:0000593", "1H nuclear magnetic resonance spectroscopy"),
    (
        "CHMO:0000595",
        "13C nuclear magnetic resonance spectroscopy",
    ),
    (
        "CHMO:0000598",
        "two-dimensional nuclear magnetic resonance spectroscopy",
    ),
    ("CHMO:0000061", "flow cytometry"),
    // ---- FBbi (imaging methods)
    ("FBbi:00000345", "light microscopy"),
    ("FBbi:00000246", "fluorescence microscopy"),
    ("FBbi:00000243", "bright-field microscopy"),
    (
        "FBbi:00000245",
        "differential interference contrast microscopy",
    ),
    ("FBbi:00000247", "phase contrast microscopy"),
    ("FBbi:00000251", "confocal microscopy"),
    ("FBbi:00000253", "spinning disk confocal microscopy"),
    ("FBbi:00000255", "multi-photon microscopy"),
    (
        "FBbi:00000369",
        "selective plane illumination fluorescence microscopy",
    ),
    ("FBbi:00000617", "evanescent wave microscopy"),
    ("FBbi:00000368", "fluorescence lifetime imaging microscopy"),
    ("FBbi:00000249", "time lapse microscopy"),
    ("FBbi:00000256", "electron microscopy"),
    ("FBbi:00000258", "transmission electron microscopy"),
    ("FBbi:00000380", "scanning-transmission electron microscopy"),
    // EPR (Bruker BES3T/ESP), checked on EBI OLS 2026-09-26
    ("CHMO:0000328", "electron spin resonance spectroscopy"),
    (
        "CHMO:0000329",
        "continuous-wave electron spin resonance spectroscopy",
    ),
    (
        "CHMO:0000330",
        "pulsed electron spin resonance spectroscopy",
    ),
    ("CHMO:0002253", "electron spin resonance spectrometer"),
    // X-ray diffraction, checked on EBI OLS 2026-09-26
    ("CHMO:0000156", "X-ray diffraction"),
    ("CHMO:0000158", "powder X-ray diffraction"),
    ("CHMO:0002105", "X-ray diffractometer"),
    // electrochemistry, checked on EBI OLS 2026-09-26
    ("CHMO:0000005", "chrono-amperometry"),
    ("CHMO:0000017", "chrono-potentiometry"),
    ("CHMO:0000025", "cyclic voltammetry"),
    ("CHMO:0000028", "linear-sweep voltammetry"),
    (
        "CHMO:0000423",
        "electrochemical-induced impedance spectroscopy",
    ),
    ("CHMO:0002427", "potentiostat"),
    ("CHMO:0002933", "open circuit voltage potentiometry"),
    (
        "CHMO:0002936",
        "galvanostatic cycling with potential limitation",
    ),
    (
        "CHMO:0002937",
        "potentiostatic electrochemical impedance spectroscopy",
    ),
    // thermal analysis, checked on EBI OLS 2026-09-26
    ("CHMO:0000681", "thermal analysis"),
    ("CHMO:0000684", "differential scanning calorimetry"),
    ("CHMO:0000690", "thermogravimetry"),
    ("CHMO:0002642", "dilatometry"),
    ("CHMO:0002200", "differential scanning calorimeter"),
    ("CHMO:0002121", "thermogravimetric analyser"),
    // rheometry (TA TRIOS), checked on EBI OLS 2026-09-26
    ("CHMO:0000915", "rheometry"),
    // light scattering (Malvern Zetasizer), checked on EBI OLS 2026-09-26
    ("CHMO:0000167", "dynamic light scattering"),
    ("CHMO:0002123", "zeta-potential measurement"),
    // ---- OBI (instruments and assays)
    ("OBI:0400169", "microscope"),
    ("OBI:0001079", "confocal microscope"),
    ("OBI:0000990", "electron microscope"),
    ("OBI:0400044", "flow cytometer"),
    ("OBI:0000049", "mass spectrometer"),
    ("OBI:0001058", "microplate reader"),
    ("OBI:0000566", "NMR instrument"),
    ("OBI:0000485", "chromatography instrument"),
    (
        "OBI:0001057",
        "high performance liquid chromatography instrument",
    ),
    ("OBI:0400115", "spectrophotometer"),
    ("OBI:0002119", "microscopy assay"),
    ("OBI:0001631", "electron microscopy imaging assay"),
    ("OBI:0000916", "flow cytometry assay"),
    ("OBI:0000470", "mass spectrometry assay"),
    (
        "OBI:0003097",
        "liquid chromatography mass spectrometry assay",
    ),
    ("OBI:0003110", "gas chromatography mass spectrometry assay"),
    (
        "OBI:0003099",
        "matrix assisted laser desorption ionization imaging mass spectrometry assay",
    ),
    ("OBI:0000623", "NMR spectroscopy assay"),
    ("OBI:0002176", "electrophysiology assay"),
    (
        "OBI:0000454",
        "extracellular electrophysiology recording assay",
    ),
    ("OBI:0003829", "luminescence detection assay"),
    ("OBI:0001501", "fluorescence detection assay"),
];

/// Units the model emits: `(symbol as written, UCUM code)`. Arbitrary units (optical density,
/// relative fluorescence) are UCUM annotations in braces.
pub const UNITS: &[(&str, &str)] = &[
    ("µm", "um"),
    ("nm", "nm"),
    ("mm", "mm"),
    ("m", "m"),
    ("Å", "Ao"),
    ("s", "s"),
    ("ms", "ms"),
    ("µs", "us"),
    ("min", "min"),
    ("Hz", "Hz"),
    ("kHz", "kHz"),
    ("MHz", "MHz"),
    ("K", "K"),
    ("°C", "Cel"),
    ("µL", "uL"),
    ("nL", "nL"),
    ("kV", "kV"),
    ("V", "V"),
    ("ppm", "[ppm]"),
    ("m/z", "{m/z}"),
    ("V·s/cm²", "V.s/cm2"),
    ("OD", "{OD}"),
    ("RFU", "{RFU}"),
    ("RLU", "{RLU}"),
    ("%", "%"),
    ("cm⁻¹", "/cm"),
    ("1/cm", "/cm"),
    ("nm/min", "nm/min"),
    ("cm", "cm"),
    ("mL", "mL"),
    ("mL/min", "mL/min"),
    ("mM", "mmol/L"),
    ("µcal/s", "ucal/s"),
    ("rpm", "{rpm}"),
    ("mm/s", "mm/s"),
    // EPR (Bruker BES3T/ESP)
    ("G", "G"),
    ("mT", "mT"),
    ("GHz", "GHz"),
    ("mW", "mW"),
    ("dB", "dB"),
    ("ns", "ns"),
    // X-ray diffraction
    ("°", "deg"),
    ("mA", "mA"),
    // electrochemistry
    ("cm²", "cm2"),
    ("mg", "mg"),
    ("mV/s", "mV/s"),
    ("mA·h", "mA.h"),
    ("A·h", "A.h"),
    ("g", "g"),
    ("A·h/g", "A.h/g"),
    ("mW·h", "mW.h"),
    ("W·h", "W.h"),
    ("Ω", "Ohm"),
    // thermal analysis
    ("µV", "uV"),
    ("N", "N"),
];

/// The curated term with this id, or `None` when it is not in [`TERMS`].
pub fn term(id: &str) -> Option<Term> {
    TERMS.iter().find(|(i, _)| *i == id).map(|(i, l)| Term {
        id: (*i).to_string(),
        label: (*l).to_string(),
    })
}

/// True when `id` is in [`TERMS`] with this label.
pub fn is_known(t: &Term) -> bool {
    TERMS.iter().any(|(i, l)| *i == t.id && *l == t.label)
}

/// UCUM code of a unit symbol from [`UNITS`].
pub fn ucum(unit: &str) -> Option<&'static str> {
    UNITS.iter().find(|(u, _)| *u == unit).map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn table_is_well_formed() {
        let mut ids = BTreeSet::new();
        for (id, label) in TERMS {
            assert!(ids.insert(*id), "duplicate term {id}");
            let (prefix, local) = id.split_once(':').expect("CURIE");
            let digits = match prefix {
                "MS" | "CHMO" | "OBI" => 7,
                "FBbi" => 8,
                _ => panic!("unknown prefix in {id}"),
            };
            assert_eq!(local.len(), digits, "{id}");
            assert!(local.bytes().all(|b| b.is_ascii_digit()), "{id}");
            assert!(!label.is_empty() && label.trim() == *label, "{id}");
        }
        let mut units = BTreeSet::new();
        for (u, c) in UNITS {
            assert!(units.insert(*u), "duplicate unit {u}");
            assert!(!c.is_empty() && !c.contains(' '), "{u}");
        }
    }

    /// Every term id written in the experiment derivation's source is in the table.
    #[test]
    fn every_term_in_the_source_is_curated() {
        let src = include_str!("experiment.rs");
        let mut n = 0;
        for (i, _) in src.match_indices('"') {
            let rest = &src[i + 1..];
            let Some(end) = rest.find('"') else { break };
            let s = &rest[..end];
            let is_id = ["MS:", "CHMO:", "FBbi:", "OBI:"]
                .iter()
                .any(|p| s.starts_with(p) && s[p.len()..].bytes().all(|b| b.is_ascii_digit()));
            if is_id {
                n += 1;
                assert!(
                    term(s).is_some(),
                    "{s} is used in experiment.rs but not in TERMS"
                );
            }
        }
        assert!(n > 20, "found only {n} term ids in experiment.rs");
    }

    #[test]
    fn lookups() {
        assert_eq!(term("MS:1000130").unwrap().label, "positive scan");
        assert!(term("MS:9999999").is_none());
        assert_eq!(ucum("µm"), Some("um"));
        assert_eq!(ucum("°C"), Some("Cel"));
        assert_eq!(ucum("furlong"), None);
    }
}
