//! Meaning of the scan-event preamble bytes and composition of the scan filter text.
//! Byte positions are the same in versions 57–66 (the preamble only grew at the end).
//! See `docs/formats/thermo-raw.md` § "Scan event preamble" for the evidence behind each byte.

use crate::layout::ScanEvent;

/// Byte positions inside the preamble.
pub const POLARITY_BYTE: usize = 4;
pub const DATA_KIND_BYTE: usize = 5;
pub const MS_LEVEL_BYTE: usize = 6;
pub const SCAN_KIND_BYTE: usize = 7;
/// In-source CID: 0 when the scan applies it, and the event's first counted tail item is its
/// energy (`sid=10.00`; `pwiz-thermo-source-cid`, `zenodo19222374-cannabis-neg-h1-1`); 1 or 2
/// in every other corpus event.
pub const SOURCE_CID_BYTE: usize = 8;
/// Scan rate: 0 prints `t` (turbo) after the ion source in ion-trap scans (two corpus files);
/// 1 and 2 print nothing (every other corpus scan). Other analyzers: not seen, nothing printed.
pub const SCAN_RATE_BYTE: usize = 9;
pub const DEPENDENT_BYTE: usize = 10;
pub const IONIZATION_BYTE: usize = 11;
/// First of two bytes printed as `{a,b}` after the analyzer when not 0xFF (bytes 28 and 30).
pub const BRACE_BYTE: usize = 28;
pub const ANALYZER_BYTE: usize = 40;
/// 0 prints `lock` before `ms` (one corpus file; 2 elsewhere).
pub const LOCK_BYTE: usize = 42;
/// 1 prints `sa` (supplemental activation) after the dependent flag: every ETD scan of the
/// ThermoRawFileParser 1.3.4 export `pxd032908-velos-etd-pep38`; 0 in the ETD scan of
/// `pwiz-thermo-bsa-ft-etd` (no `sa`) and elsewhere. Absent from preambles of fewer than 121
/// bytes (file versions before 63).
pub const SUPPLEMENTAL_ACTIVATION_BYTE: usize = 120;
/// FAIMS: 0 when the scan was taken behind a FAIMS device, and a counted tail item (after the
/// in-source CID energy, when there is one) is the compensation voltage (`cv=-40.00`; every event
/// of the Orbitrap Astral and Eclipse FAIMS files); 1 or 2 elsewhere.
pub const FAIMS_BYTE: usize = 122;

/// How an instrument generation numbers its ion-trap scan rates (preamble byte 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanRates {
    /// LTQ-generation ion traps (LTQ XL, LTQ Velos, Stellar): 0 turbo, others print nothing.
    Ltq,
    /// Tribrid ion traps (Orbitrap Fusion, Fusion Lumos, Eclipse, ID-X, IQ-X, Ascend): 0 turbo,
    /// 1 rapid.
    Tribrid,
}

impl ScanRates {
    /// The numbering an instrument model uses.
    pub fn for_model(model: Option<&str>) -> ScanRates {
        let m = model.unwrap_or("").to_ascii_lowercase();
        if ["fusion", "eclipse", "id-x", "iq-x", "ascend", "tribrid"]
            .iter()
            .any(|k| m.contains(k))
        {
            ScanRates::Tribrid
        } else {
            ScanRates::Ltq
        }
    }
}

/// Ion polarity of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Negative,
    Positive,
    Unknown,
}

impl Polarity {
    pub fn word(self) -> &'static str {
        match self {
            Polarity::Negative => "negative",
            Polarity::Positive => "positive",
            Polarity::Unknown => "unknown",
        }
    }
    pub fn sign(self) -> &'static str {
        match self {
            Polarity::Negative => "-",
            Polarity::Positive => "+",
            Polarity::Unknown => "?",
        }
    }
}

/// Mass analyzer that recorded the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Analyzer {
    IonTrap,
    TripleQuadrupole,
    SingleQuadrupole,
    TimeOfFlight,
    FourierTransform,
    Sector,
    /// The Orbitrap Astral's second analyzer (code 7; filter token `ASTMS` in the exports).
    Astral,
    Unknown(u8),
}

impl Analyzer {
    pub fn from_code(b: u8) -> Self {
        match b {
            0 => Analyzer::IonTrap,
            1 => Analyzer::TripleQuadrupole,
            2 => Analyzer::SingleQuadrupole,
            3 => Analyzer::TimeOfFlight,
            4 => Analyzer::FourierTransform,
            5 => Analyzer::Sector,
            7 => Analyzer::Astral,
            x => Analyzer::Unknown(x),
        }
    }
    /// The token that opens a scan filter.
    pub fn filter_token(self) -> Option<&'static str> {
        Some(match self {
            Analyzer::IonTrap => "ITMS",
            Analyzer::TripleQuadrupole => "TQMS",
            Analyzer::SingleQuadrupole => "SQMS",
            Analyzer::TimeOfFlight => "TOFMS",
            Analyzer::FourierTransform => "FTMS",
            Analyzer::Sector => "Sector",
            Analyzer::Astral => "ASTMS",
            Analyzer::Unknown(_) => return None,
        })
    }
}

/// Ion source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ionization {
    ElectronImpact,
    ChemicalIonization,
    FastAtomBombardment,
    Electrospray,
    AtmosphericPressureChemical,
    Nanospray,
    Thermospray,
    FieldDesorption,
    Maldi,
    GlowDischarge,
    Unknown(u8),
}

impl Ionization {
    pub fn from_code(b: u8) -> Self {
        match b {
            0 => Ionization::ElectronImpact,
            1 => Ionization::ChemicalIonization,
            2 => Ionization::FastAtomBombardment,
            3 => Ionization::Electrospray,
            4 => Ionization::AtmosphericPressureChemical,
            5 => Ionization::Nanospray,
            6 => Ionization::Thermospray,
            7 => Ionization::FieldDesorption,
            8 => Ionization::Maldi,
            9 => Ionization::GlowDischarge,
            x => Ionization::Unknown(x),
        }
    }
    pub fn filter_token(self) -> Option<&'static str> {
        Some(match self {
            Ionization::ElectronImpact => "EI",
            Ionization::ChemicalIonization => "CI",
            Ionization::FastAtomBombardment => "FAB",
            Ionization::Electrospray => "ESI",
            Ionization::AtmosphericPressureChemical => "APCI",
            Ionization::Nanospray => "NSI",
            Ionization::Thermospray => "TSP",
            Ionization::FieldDesorption => "FD",
            Ionization::Maldi => "MALDI",
            Ionization::GlowDischarge => "GD",
            Ionization::Unknown(_) => return None,
        })
    }
}

/// Fragmentation method of an MS^n stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    BeamCollision,
    TrapCollision,
    ElectronTransfer,
    Unknown(u32),
}

impl Activation {
    /// From the first word of a reaction record.
    pub fn from_reaction_code(code: u32) -> Self {
        match code {
            1 => Activation::TrapCollision,
            9 => Activation::ElectronTransfer,
            11 => Activation::BeamCollision,
            x => Activation::Unknown(x),
        }
    }
    /// Lower-case token used after `@` in scan filters.
    pub fn filter_token(self) -> Option<&'static str> {
        match self {
            Activation::BeamCollision => Some("hcd"),
            Activation::TrapCollision => Some("cid"),
            Activation::ElectronTransfer => Some("etd"),
            Activation::Unknown(_) => None,
        }
    }
}

/// Scan type token.
pub fn scan_kind_token(b: u8) -> Option<&'static str> {
    Some(match b {
        0 => "Full",
        1 => "Z",
        2 => "SIM",
        3 => "SRM",
        4 => "CRM",
        6 => "Q1MS",
        7 => "Q3MS",
        _ => return None,
    })
}

/// `v` with `decimals` fraction digits as the instrument software prints filter masses: the
/// value written with 15 significant digits, rounded half away from zero. An exact binary tie
/// (`309.03125` → `309.0313` at 4 decimals) and a value stored just below a decimal tie
/// (`790.60925`, stored as 790.6092499999999745… → `790.6093`) both round up; Rust's `{:.4}`
/// gives `309.0312` and `790.6092`. Other values print as the standard formatting does.
pub fn fixed_decimals(v: f64, decimals: usize) -> String {
    let standard = format!("{v:.decimals$}");
    if !v.is_finite() {
        return standard;
    }
    // `d.dddddddddddddde<exp>`: 15 significant digits and a power of ten.
    let sci = format!("{:.14e}", v.abs());
    let Some((mantissa, exp)) = sci.split_once('e') else {
        return standard;
    };
    let Ok(exp) = exp.parse::<i64>() else {
        return standard;
    };
    let mut digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    // Digits before the decimal point: at least one (a leading zero for values below 1).
    let mut int_len = exp + 1;
    if int_len < 1 {
        let zeros = usize::try_from(1 - int_len).unwrap_or(0);
        digits.splice(0..0, std::iter::repeat_n(0, zeros));
        int_len = 1;
    }
    let int_len = usize::try_from(int_len).unwrap_or(1);
    let keep = int_len + decimals;
    let round_up = digits.get(keep).is_some_and(|&d| d >= 5);
    digits.resize(keep, 0);
    let mut int_len = int_len;
    if round_up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                int_len += 1;
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let text = |d: &[u8]| d.iter().map(|&x| char::from(b'0' + x)).collect::<String>();
    let mut body = text(&digits[..int_len]);
    if decimals > 0 {
        body.push('.');
        body.push_str(&text(&digits[int_len..]));
    }
    if v < 0.0 { format!("-{body}") } else { body }
}

fn byte(e: &ScanEvent, i: usize) -> u8 {
    e.preamble.get(i).copied().unwrap_or(0xFF)
}

impl ScanEvent {
    pub fn polarity(&self) -> Polarity {
        match byte(self, POLARITY_BYTE) {
            0 => Polarity::Negative,
            1 => Polarity::Positive,
            _ => Polarity::Unknown,
        }
    }
    /// `Some(true)` for profile data, `Some(false)` for centroids.
    pub fn is_profile(&self) -> Option<bool> {
        match byte(self, DATA_KIND_BYTE) {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    /// MS level (1 = full scan); 0 when undefined.
    pub fn ms_level(&self) -> u32 {
        match byte(self, MS_LEVEL_BYTE) {
            b @ 1..=16 => u32::from(b),
            _ => 0,
        }
    }
    pub fn scan_kind_code(&self) -> u8 {
        byte(self, SCAN_KIND_BYTE)
    }
    pub fn is_dependent(&self) -> bool {
        byte(self, DEPENDENT_BYTE) == 1
    }
    pub fn ionization(&self) -> Ionization {
        Ionization::from_code(byte(self, IONIZATION_BYTE))
    }
    pub fn analyzer(&self) -> Analyzer {
        Analyzer::from_code(byte(self, ANALYZER_BYTE))
    }

    /// The reactions that belong to MS^2..MS^n stages, in filter order. Version-66 MS1 events
    /// store one full-range isolation record that is not a precursor. An MS^n event with more
    /// reactions than stages is a multiplexed (`msx`) scan: its first stage isolates several
    /// precursors, all listed.
    pub fn precursor_reactions(&self) -> &[crate::layout::Reaction] {
        if self.ms_level() < 2 {
            return &[];
        }
        &self.reactions
    }

    /// A multiplexed (`msx`) scan: more reactions than MS^n stages.
    pub fn is_multiplex(&self) -> bool {
        self.ms_level() >= 2 && self.reactions.len() > (self.ms_level() - 1) as usize
    }

    /// In-source CID energy (`sid=`): when [`SOURCE_CID_BYTE`] is 0, the event's first counted
    /// 8-byte tail item as an f64.
    pub fn source_cid_energy(&self) -> Option<f64> {
        if byte(self, SOURCE_CID_BYTE) != 0 {
            return None;
        }
        self.tail_items
            .first()
            .map(|&bits| f64::from_bits(bits))
            .filter(|v| v.is_finite())
    }

    /// FAIMS compensation voltage (`cv=`, volts): when [`FAIMS_BYTE`] is 0, the tail item after
    /// the in-source CID energy (the first item when the scan has none).
    pub fn faims_cv(&self) -> Option<f64> {
        if byte(self, FAIMS_BYTE) != 0 {
            return None;
        }
        let at = usize::from(byte(self, SOURCE_CID_BYTE) == 0);
        self.tail_items
            .get(at)
            .map(|&bits| f64::from_bits(bits))
            .filter(|v| v.is_finite())
    }

    /// `t` (turbo scan rate) or nothing, as LTQ-generation ion traps number their rates.
    pub fn scan_rate_token(&self) -> Option<&'static str> {
        self.scan_rate_token_for(ScanRates::Ltq)
    }

    /// The ion-trap scan-rate token: 0 is `t` (turbo); for Tribrid ion traps 1 is `r` (rapid),
    /// which LTQ-generation traps print as nothing.
    pub fn scan_rate_token_for(&self, rates: ScanRates) -> Option<&'static str> {
        if self.analyzer() != Analyzer::IonTrap {
            return None;
        }
        match (rates, byte(self, SCAN_RATE_BYTE)) {
            (_, 0) => Some("t"),
            (ScanRates::Tribrid, 1) => Some("r"),
            _ => None,
        }
    }

    /// Compose the scan filter text, e.g. `FTMS + p ESI d Full ms2 84.08@cid20.00 [50.00-95.00]`.
    /// `mass_decimals` comes from the run header (2 or 4 in the corpus).
    pub fn filter_text(&self, mass_decimals: usize) -> String {
        self.filter_text_for(mass_decimals, ScanRates::Ltq)
    }

    /// [`ScanEvent::filter_text`] for an instrument whose ion trap numbers scan rates `rates`.
    pub fn filter_text_for(&self, mass_decimals: usize, rates: ScanRates) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = self.analyzer().filter_token() {
            parts.push(t.into());
        }
        let (b1, b2) = (byte(self, BRACE_BYTE), byte(self, BRACE_BYTE + 2));
        if b1 != 0xFF && b2 != 0xFF {
            // Printed with two spaces after it in the reference filter text.
            parts.push(format!("{{{b1},{b2}}} "));
        }
        parts.push(self.polarity().sign().into());
        match self.is_profile() {
            Some(true) => parts.push("p".into()),
            Some(false) => parts.push("c".into()),
            None => {}
        }
        if let Some(t) = self.ionization().filter_token() {
            parts.push(t.into());
        }
        if let Some(e) = self.source_cid_energy() {
            parts.push(format!("sid={}", fixed_decimals(e, 2)));
        }
        if let Some(v) = self.faims_cv() {
            parts.push(format!("cv={}", fixed_decimals(v, 2)));
        }
        if let Some(t) = self.scan_rate_token_for(rates) {
            parts.push(t.into());
        }
        if self.is_dependent() {
            parts.push("d".into());
        }
        if byte(self, SUPPLEMENTAL_ACTIVATION_BYTE) == 1 {
            parts.push("sa".into());
        }
        if let Some(t) = scan_kind_token(self.scan_kind_code()) {
            parts.push(t.into());
        }
        if byte(self, LOCK_BYTE) == 0 {
            parts.push("lock".into());
        }
        let level = self.ms_level();
        if self.is_multiplex() {
            parts.push("msx".into());
        }
        parts.push(if level > 1 {
            format!("ms{level}")
        } else {
            "ms".into()
        });
        for r in self.precursor_reactions() {
            // SRM/CRM filters name only the precursor (inferred; the TSQ file's reaction code
            // is 12, which no reference conversion names).
            match r.activation().filter_token() {
                Some(act) if !matches!(self.scan_kind_code(), 3 | 4) => parts.push(format!(
                    "{}@{act}{}",
                    fixed_decimals(r.precursor_mz, mass_decimals),
                    fixed_decimals(r.energy, 2),
                )),
                _ => parts.push(fixed_decimals(r.precursor_mz, mass_decimals)),
            }
        }
        let windows: Vec<String> = self
            .scan_ranges
            .iter()
            .map(|r| {
                format!(
                    "{}-{}",
                    fixed_decimals(r[0], mass_decimals),
                    fixed_decimals(r[1], mass_decimals)
                )
            })
            .collect();
        parts.push(format!("[{}]", windows.join(", ")));
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_masses_round_ties_away_from_zero() {
        assert_eq!(fixed_decimals(309.031_25, 4), "309.0313");
        assert_eq!(fixed_decimals(250.906_25, 4), "250.9063");
        assert_eq!(fixed_decimals(-0.125, 2), "-0.13");
        // stored just below the decimal tie: 15 significant digits make it a tie, rounded up
        // (790.60925 prints 790.6093 in the Astral export `msv99294-astral-nanopots-1cell-b9`)
        assert_eq!(fixed_decimals(790.609_25, 4), "790.6093");
        assert_eq!(fixed_decimals(9.995, 2), "10.00");
        assert_eq!(fixed_decimals(0.001_234, 2), "0.00");
        assert_eq!(fixed_decimals(0.005, 2), "0.01");
        assert_eq!(fixed_decimals(1.0e-20, 4), "0.0000");
        assert_eq!(fixed_decimals(123_456.0, 0), "123456");
        assert_eq!(fixed_decimals(-45.0, 2), "-45.00");
        assert_eq!(fixed_decimals(99.968_75, 4), "99.9688");
        assert_eq!(fixed_decimals(99.999_96, 4), "100.0000"); // not a tie: standard rounding
        assert_eq!(fixed_decimals(0.5, 0), "1");
        assert_eq!(fixed_decimals(9.5, 0), "10");
        assert_eq!(fixed_decimals(84.08, 2), "84.08");
        assert_eq!(fixed_decimals(f64::NAN, 2), "NaN");
    }
    use crate::layout::Reaction;

    fn event(pre: &[(usize, u8)], reactions: Vec<Reaction>, range: [f64; 2]) -> ScanEvent {
        let mut preamble = vec![0u8; 128];
        preamble[28..32].fill(0xFF);
        preamble[42] = 2;
        for &(i, b) in pre {
            preamble[i] = b;
        }
        ScanEvent {
            preamble,
            reactions,
            scan_ranges: vec![range],
            ..Default::default()
        }
    }

    #[test]
    fn corpus_filter_examples() {
        // mtbls404: FTMS - p ESI Full ms [50.00-1000.00]
        let e = event(
            &[(4, 0), (5, 1), (6, 1), (7, 0), (10, 0), (11, 3), (40, 4)],
            vec![],
            [50.0, 1000.0],
        );
        assert_eq!(e.filter_text(2), "FTMS - p ESI Full ms [50.00-1000.00]");
        // mtbls20: FTMS + p ESI d Full ms2 84.08@cid20.00 [50.00-95.00]
        let r = Reaction {
            precursor_mz: 84.080_337_524_414_06,
            isolation_width: 1.0,
            energy: 20.0,
            reaction_words: [1, 0],
            reaction_values: [0.0; 3],
        };
        let e = event(
            &[(4, 1), (5, 1), (6, 2), (10, 1), (11, 3), (24, 4), (40, 4)],
            vec![r],
            [50.0, 95.0],
        );
        assert_eq!(
            e.filter_text(2),
            "FTMS + p ESI d Full ms2 84.08@cid20.00 [50.00-95.00]"
        );
        assert_eq!(e.polarity(), Polarity::Positive);
        assert_eq!(e.ms_level(), 2);
        // mtbls797: FTMS {1,1}  + p ESI Full lock ms [150.00-1500.00]
        let e = event(
            &[
                (4, 1),
                (5, 1),
                (6, 1),
                (10, 2),
                (11, 3),
                (28, 1),
                (29, 0),
                (30, 1),
                (31, 0),
                (40, 4),
                (42, 0),
            ],
            vec![],
            [150.0, 1500.0],
        );
        assert_eq!(
            e.filter_text(2),
            "FTMS {1,1}  + p ESI Full lock ms [150.00-1500.00]"
        );
    }

    #[test]
    fn srm_filter_lists_every_window() {
        // mtbls1822 scan 1: preamble bytes as stored, reaction code 12, two product windows.
        let r = Reaction {
            precursor_mz: 258.026_000_976_562_5,
            isolation_width: 1.0,
            energy: 25.0,
            reaction_words: [12, 0],
            reaction_values: [0.0; 3],
        };
        let mut e = event(
            &[(4, 0), (5, 0), (6, 2), (7, 3), (10, 0), (11, 3), (40, 6)],
            vec![r],
            [0.0, 0.0],
        );
        e.scan_ranges = vec![
            [78.942_001_342_773_44, 78.944_000_244_140_62],
            [96.939_002_990_722_66, 96.941_001_892_089_84],
        ];
        assert_eq!(
            e.filter_text(3),
            "- c ESI SRM ms2 258.026 [78.942-78.944, 96.939-96.941]"
        );
        assert_eq!(
            e.scan_range().map(f64::to_bits),
            [
                78.942_001_342_773_44_f64.to_bits(),
                96.941_001_892_089_84_f64.to_bits()
            ]
        );
        assert_eq!(e.ms_level(), 2);
    }

    #[test]
    fn faims_and_astral_filters() {
        // msv99294 scan 2: an Astral-analyzer MS2 behind FAIMS; tail item -40.0, byte 122 = 0.
        let r = Reaction {
            precursor_mz: 410.436_462_402_343_75,
            isolation_width: 20.009_094,
            energy: 25.0,
            reaction_words: [11, 1],
            reaction_values: [400.431_92, 420.441_01, 0.0],
        };
        let mut e = event(
            &[
                (4, 1),
                (5, 0),
                (6, 2),
                (8, 1),
                (9, 2),
                (10, 0),
                (11, 5),
                (40, 7),
                (122, 0),
            ],
            vec![r],
            [150.0, 2000.0],
        );
        e.tail_items = vec![(-40.0f64).to_bits()];
        assert_eq!(e.analyzer(), Analyzer::Astral);
        assert_eq!(e.faims_cv(), Some(-40.0));
        assert_eq!(e.source_cid_energy(), None);
        assert_eq!(
            e.filter_text(4),
            "ASTMS + c NSI cv=-40.00 Full ms2 410.4365@hcd25.00 [150.0000-2000.0000]"
        );
        // msv99508: an Orbitrap MS2 of a dependent scan behind FAIMS.
        e.preamble[ANALYZER_BYTE] = 4;
        e.preamble[DEPENDENT_BYTE] = 1;
        e.tail_items = vec![(-45.0f64).to_bits()];
        e.reactions[0].precursor_mz = 442.682_8;
        e.reactions[0].energy = 32.0;
        e.scan_ranges = vec![[110.0, 896.0]];
        assert_eq!(
            e.filter_text(4),
            "FTMS + c NSI cv=-45.00 d Full ms2 442.6828@hcd32.00 [110.0000-896.0000]"
        );
        // pwiz-thermo-source-cid: byte 8 = 0 makes the first tail item the in-source CID energy;
        // byte 122 = 1 means no FAIMS.
        e.preamble[SOURCE_CID_BYTE] = 0;
        e.preamble[FAIMS_BYTE] = 1;
        e.tail_items = vec![10.0f64.to_bits()];
        assert_eq!(e.source_cid_energy(), Some(10.0));
        assert_eq!(e.faims_cv(), None);
        // A Q Exactive event: byte 8 = 1 and a zero tail item print neither.
        e.preamble[SOURCE_CID_BYTE] = 1;
        e.preamble[FAIMS_BYTE] = 2;
        e.tail_items = vec![0.0f64.to_bits()];
        assert_eq!((e.source_cid_energy(), e.faims_cv()), (None, None));
    }
}
