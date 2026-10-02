//! The vendor-neutral model every export parser fills: an export holds plate reads (blocks),
//! each with one or more measurement channels and one observation per well, channel, time
//! point and wavelength.

use std::collections::BTreeMap;

use openreadout_core::model::Finding;
use serde_json::{Value, json};

use crate::sheet::Container;

/// Most rows or columns a plate may have. The largest standard plate is 48 x 72 (3456 wells);
/// non-standard layouts are sized from the wells seen, up to this.
pub(crate) const MAX_PLATE_DIM: u32 = 1024;

/// Which exporter wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Gen5,
    SoftMaxPro,
    BmgMars,
    BmgSmartControl,
    EnVision,
    Kaleido,
    TecanIControl,
    TecanMagellan,
    SkanIt,
    Generic,
}

impl Kind {
    /// Our id for the export dialect (table `extra.export`).
    pub(crate) fn id(self) -> &'static str {
        match self {
            Kind::Gen5 => "gen5",
            Kind::SoftMaxPro => "softmax-pro",
            Kind::BmgMars => "bmg-mars",
            Kind::BmgSmartControl => "bmg-smart-control",
            Kind::EnVision => "envision",
            Kind::Kaleido => "kaleido",
            Kind::TecanIControl => "tecan-i-control",
            Kind::TecanMagellan => "tecan-magellan",
            Kind::SkanIt => "skanit",
            Kind::Generic => "generic",
        }
    }
    /// Instrument manufacturer (nominative use).
    pub(crate) fn manufacturer(self) -> Option<&'static str> {
        Some(match self {
            Kind::Gen5 => "Agilent BioTek",
            Kind::SoftMaxPro => "Molecular Devices",
            Kind::BmgMars | Kind::BmgSmartControl => "BMG LABTECH",
            Kind::EnVision | Kind::Kaleido => "Revvity (PerkinElmer)",
            Kind::TecanIControl | Kind::TecanMagellan => "Tecan",
            Kind::SkanIt => "Thermo Fisher Scientific",
            Kind::Generic => return None,
        })
    }
    /// The software that writes this export.
    pub(crate) fn software(self) -> Option<&'static str> {
        Some(match self {
            Kind::Gen5 => "Gen5",
            Kind::SoftMaxPro => "SoftMax Pro",
            Kind::BmgMars => "MARS",
            Kind::BmgSmartControl => "SMART Control",
            Kind::EnVision => "EnVision Workstation",
            Kind::Kaleido => "Kaleido",
            Kind::TecanIControl => "i-control",
            Kind::TecanMagellan => "Magellan",
            Kind::SkanIt => "SkanIt",
            Kind::Generic => return None,
        })
    }
}

/// Detection mode of a measurement channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Mode {
    Absorbance,
    Fluorescence,
    Luminescence,
    /// AlphaScreen / AlphaLISA (laser-excited luminescent proximity assays).
    Alpha,
    Unknown,
}

impl Mode {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Mode::Absorbance => "absorbance",
            Mode::Fluorescence => "fluorescence",
            Mode::Luminescence => "luminescence",
            Mode::Alpha => "alpha",
            Mode::Unknown => "unknown",
        }
    }
    /// Guess the mode from free text (`Absorbance Endpoint`, `Fluorescence (FI)`, `Lum`, `OD`).
    pub(crate) fn from_text(s: &str) -> Mode {
        let l = s.to_ascii_lowercase();
        // `OD`, `OD600`, `A450` as words
        let od_word = l.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
            let digits = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
            w == "od"
                || w.strip_prefix("od").is_some_and(digits)
                || (w.len() == 4 && w.strip_prefix('a').is_some_and(digits))
        });
        if l.contains("absorb") || l.starts_with("abs") || od_word || l.contains("optical density")
        {
            Mode::Absorbance
        } else if l.contains("alpha") {
            Mode::Alpha
        } else if l.contains("fluor")
            || l.contains("htrf")
            || l.contains("trf")
            || l == "fi"
            || l.contains("(fi")
            || l.contains("polariz")
        {
            Mode::Fluorescence
        } else if l.contains("lum") {
            Mode::Luminescence
        } else {
            Mode::Unknown
        }
    }
}

/// How a read's detection mode was established: read from a field of the export that names it,
/// or derived by a rule (`docs/formats/plate-readers.md` § Read modes). Derived modes are listed
/// in the file's `assurance.inferred`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ModeBasis {
    /// A field of the export names the mode (BMG `Absorbance`, Gen5 `Read Absorbance Endpoint`,
    /// SoftMax Pro `ReadMode`, i-control `Mode`).
    Stated,
    /// Derived from the detector the export records for the read (EnVision `MeasInfo` `De=`).
    Detector,
    /// Derived from the read's label or technology name by keywords (`US LUM 384` → luminescence,
    /// `485,528` → fluorescence).
    Label,
    /// Derived from the read's settings (a `Measurement wavelength` and no excitation or
    /// emission: absorbance).
    Settings,
    /// Nothing names the mode: it is `unknown`.
    Undetermined,
}

impl ModeBasis {
    pub(crate) fn name(self) -> &'static str {
        match self {
            ModeBasis::Stated => "stated",
            ModeBasis::Detector => "detector",
            ModeBasis::Label => "label",
            ModeBasis::Settings => "settings",
            ModeBasis::Undetermined => "undetermined",
        }
    }
    /// The basis of `mode` when it came from `basis`: `unknown` is always undetermined.
    pub(crate) fn of(mode: Mode, basis: ModeBasis) -> ModeBasis {
        if mode == Mode::Unknown {
            ModeBasis::Undetermined
        } else {
            basis
        }
    }
}

/// How the plate was read over time or wavelength.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadType {
    Endpoint,
    Kinetic,
    Spectrum,
}

impl ReadType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            ReadType::Endpoint => "endpoint",
            ReadType::Kinetic => "kinetic",
            ReadType::Spectrum => "spectrum",
        }
    }
}

/// One measurement channel of a plate read (a wavelength, a filter pair, a label).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Channel {
    /// The channel as the file names it (`od:600`, `Raw Data (580/620)`, `Label1`).
    pub(crate) label: String,
    pub(crate) mode: Mode,
    /// How `mode` was established.
    pub(crate) mode_basis: ModeBasis,
    /// Absorbance measurement wavelength, or the detection wavelength when only one is given.
    pub(crate) wavelength_nm: Option<f64>,
    pub(crate) excitation_nm: Option<f64>,
    pub(crate) emission_nm: Option<f64>,
    /// Unit of the values as the file states it (`OD`, `RFU`, `RLU`, `Counts`, `CPS`).
    pub(crate) unit: Option<String>,
    /// Values derived by the vendor software (ratios, blank correction) rather than read.
    pub(crate) calculated: bool,
    /// The vendor software's formula for a calculated read, when the export stores it.
    pub(crate) formula: Option<String>,
    /// Read settings in our vocabulary (`gain`, `flashes`, `read_height_mm`, `optics`, …).
    pub(crate) settings: BTreeMap<String, Value>,
}

impl Channel {
    pub(crate) fn new(label: impl Into<String>, mode: Mode) -> Self {
        Channel {
            label: label.into(),
            mode,
            mode_basis: ModeBasis::of(mode, ModeBasis::Stated),
            wavelength_nm: None,
            excitation_nm: None,
            emission_nm: None,
            unit: None,
            calculated: false,
            formula: None,
            settings: BTreeMap::new(),
        }
    }
    /// A channel whose mode was derived by `basis` (a rule), not read from a mode field.
    pub(crate) fn derived(label: impl Into<String>, mode: Mode, basis: ModeBasis) -> Self {
        let mut c = Channel::new(label, mode);
        c.mode_basis = ModeBasis::of(mode, basis);
        c
    }
    /// Set the mode and how it was established.
    pub(crate) fn set_mode(&mut self, mode: Mode, basis: ModeBasis) {
        self.mode = mode;
        self.mode_basis = ModeBasis::of(mode, basis);
    }
    /// Short description for the `read` column label: `2 = NormLum (calculated by the vendor
    /// software, not measured)`.
    pub(crate) fn summary(&self, index: usize) -> String {
        let label = if self.label.is_empty() {
            "unlabelled"
        } else {
            self.label.as_str()
        };
        if self.calculated {
            format!(
                "{} = {label} (CALCULATED by the vendor software, not measured{})",
                index + 1,
                self.formula
                    .as_deref()
                    .map(|f| format!("; formula: {f}"))
                    .unwrap_or_default()
            )
        } else {
            format!("{} = {label} ({}, measured)", index + 1, self.mode.name())
        }
    }

    /// The wavelength recorded in the long-form table for this channel.
    pub(crate) fn detection_nm(&self) -> Option<f64> {
        self.emission_nm.or(self.wavelength_nm)
    }
    pub(crate) fn describe(&self, index: usize) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("read".into(), json!(index + 1));
        m.insert("label".into(), json!(self.label));
        m.insert("mode".into(), json!(self.mode.name()));
        if !self.calculated {
            m.insert("mode_basis".into(), json!(self.mode_basis.name()));
        }
        for (k, v) in [
            ("wavelength_nm", self.wavelength_nm),
            ("excitation_nm", self.excitation_nm),
            ("emission_nm", self.emission_nm),
        ] {
            if let Some(v) = v {
                m.insert(k.into(), json!(v));
            }
        }
        if let Some(u) = &self.unit {
            m.insert("unit".into(), json!(u));
        }
        if self.calculated {
            m.insert("calculated".into(), json!(true));
        }
        m.insert(
            "origin".into(),
            json!(if self.calculated {
                "calculated"
            } else {
                "measured"
            }),
        );
        if let Some(f) = &self.formula {
            m.insert("formula".into(), json!(f));
        }
        if !self.settings.is_empty() {
            m.insert("settings".into(), json!(self.settings));
        }
        Value::Object(m)
    }
}

/// One value: a well, a channel, and optionally a time point and a wavelength.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Obs {
    /// Zero-based plate row and column.
    pub(crate) row: u32,
    pub(crate) col: u32,
    /// Zero-based index into `Block::channels`.
    pub(crate) channel: u32,
    pub(crate) time_s: Option<f64>,
    /// Row-level wavelength (spectral scans); otherwise the channel's.
    pub(crate) wavelength_nm: Option<f64>,
    /// NaN when the cell was not a number.
    pub(crate) value: f64,
    /// The cell text when it was not a number (`OVRFLW`, `Range?`, `NaN`).
    pub(crate) text: Option<String>,
}

/// One plate read: a table.
#[derive(Debug, Clone, Default)]
pub(crate) struct Block {
    pub(crate) name: String,
    /// Plate identity used to group reads of the same physical plate (name, number or barcode).
    pub(crate) plate: String,
    pub(crate) barcode: Option<String>,
    pub(crate) plate_type: Option<String>,
    /// Plate geometry (rows × columns).
    pub(crate) rows: u32,
    pub(crate) cols: u32,
    /// Well count the file declares for the plate type, when it does.
    pub(crate) declared_wells: Option<u32>,
    pub(crate) read_type: Option<ReadType>,
    pub(crate) channels: Vec<Channel>,
    pub(crate) obs: Vec<Obs>,
    pub(crate) temperature_c: Option<f64>,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) extra: BTreeMap<String, Value>,
    pub(crate) findings: Vec<Finding>,
    /// Accept `0,02` as 0.02 (tab-delimited dialects only; counted in `extra.decimal_comma_values`).
    pub(crate) decimal_comma: bool,
    /// Where the block is in the file (sheet name and first line), for `info --view structure`.
    pub(crate) sheet: Option<String>,
    pub(crate) line: Option<usize>,
}

impl Block {
    pub(crate) fn new(name: impl Into<String>, plate: impl Into<String>) -> Self {
        Block {
            name: name.into(),
            plate: plate.into(),
            ..Default::default()
        }
    }
    /// Add a channel, returning its index; an identical channel is reused.
    pub(crate) fn channel(&mut self, ch: Channel) -> u32 {
        if let Some(i) = self.channels.iter().position(|c| *c == ch) {
            return i as u32;
        }
        self.channels.push(ch);
        (self.channels.len() - 1) as u32
    }
    /// Record a cell: numbers as values, other non-blank text as NaN with the text kept.
    pub(crate) fn push_cell(
        &mut self,
        row: u32,
        col: u32,
        channel: u32,
        time_s: Option<f64>,
        wavelength_nm: Option<f64>,
        cell: &crate::sheet::Cell,
    ) {
        if cell.is_blank() {
            return;
        }
        // Row and column come from header fields and labels; a damaged export can place a
        // well at row 4 billion, which would size the plate (and its well-name list) from it.
        if row >= MAX_PLATE_DIM || col >= MAX_PLATE_DIM {
            if !self.findings.iter().any(|f| f.code == "well_out_of_range") {
                self.findings.push(Finding::warning(
                    "well_out_of_range",
                    format!(
                        "{}: a value at row {} column {} lies beyond any plate ({MAX_PLATE_DIM} rows or columns); such values are skipped",
                        self.name,
                        u64::from(row) + 1,
                        u64::from(col) + 1
                    ),
                ));
            }
            return;
        }
        let (value, text) = match cell.number() {
            Some(v) => (v, None),
            None => match self
                .decimal_comma
                .then(|| decimal_comma(&cell.trimmed()))
                .flatten()
            {
                Some(v) => {
                    let n = self
                        .extra
                        .get("decimal_comma_values")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    self.extra
                        .insert("decimal_comma_values".into(), json!(n + 1));
                    (v, None)
                }
                None => (f64::NAN, Some(cell.trimmed())),
            },
        };
        self.obs.push(Obs {
            row,
            col,
            channel,
            time_s,
            wavelength_nm,
            value,
            text,
        });
    }
    /// Record a decoded number (binary documents); `text` names why a value is missing.
    pub(crate) fn push_value(
        &mut self,
        row: u32,
        col: u32,
        channel: u32,
        time_s: Option<f64>,
        value: f64,
        text: Option<String>,
    ) {
        if row >= MAX_PLATE_DIM || col >= MAX_PLATE_DIM {
            return;
        }
        self.obs.push(Obs {
            row,
            col,
            channel,
            time_s,
            wavelength_nm: None,
            value,
            text,
        });
    }
    /// Fix the geometry from the declared well count, else the smallest standard plate that
    /// holds every observed well.
    pub(crate) fn settle_geometry(&mut self) {
        if let Some((r, c)) = self.declared_wells.and_then(crate::grid::dims_for_wells) {
            let fits = self.obs.iter().all(|o| o.row < r && o.col < c);
            if fits {
                self.rows = r;
                self.cols = c;
                return;
            }
        }
        let max_r = self.obs.iter().map(|o| o.row).max().unwrap_or(0);
        let max_c = self.obs.iter().map(|o| o.col).max().unwrap_or(0);
        let (r, c) = crate::grid::smallest_plate(
            max_r.max(self.rows.saturating_sub(1)),
            max_c.max(self.cols.saturating_sub(1)),
        );
        self.rows = r;
        self.cols = c;
    }
    /// Sort observations: channel, time, wavelength, row, column.
    pub(crate) fn sort(&mut self) {
        let key = |o: &Obs| {
            (
                o.channel,
                o.time_s.unwrap_or(-1.0),
                o.wavelength_nm.unwrap_or(-1.0),
                o.row,
                o.col,
            )
        };
        self.obs.sort_by(|a, b| {
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0)
                .then(ka.1.total_cmp(&kb.1))
                .then(ka.2.total_cmp(&kb.2))
                .then(ka.3.cmp(&kb.3))
                .then(ka.4.cmp(&kb.4))
        });
    }
    pub(crate) fn well_count(&self) -> usize {
        let mut w: Vec<(u32, u32)> = self.obs.iter().map(|o| (o.row, o.col)).collect();
        w.sort_unstable();
        w.dedup();
        w.len()
    }
    pub(crate) fn time_points(&self) -> Vec<f64> {
        let mut t: Vec<f64> = self.obs.iter().filter_map(|o| o.time_s).collect();
        t.sort_by(f64::total_cmp);
        t.dedup();
        t
    }
}

/// A parsed export.
#[derive(Debug, Clone)]
pub(crate) struct Export {
    pub(crate) kind: Kind,
    pub(crate) container: Container,
    pub(crate) model: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) software_version: Option<String>,
    pub(crate) protocol: Option<String>,
    pub(crate) experiment: Option<String>,
    pub(crate) operator: Option<String>,
    pub(crate) acquired_at: Option<String>,
    pub(crate) acquired_raw: Option<String>,
    /// When the file was saved or exported, when that is the only time it states (not a read
    /// time), and the text it was read from.
    pub(crate) saved_raw: Option<String>,
    pub(crate) date_order_assumed: bool,
    /// Header key/value pairs as the file writes them, in order (the `vendor` tree).
    pub(crate) header: Vec<(String, String)>,
    /// Other vendor sections kept verbatim (procedure text, protocol parameters, …).
    pub(crate) sections: BTreeMap<String, Value>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) notes: Vec<String>,
}

impl Export {
    pub(crate) fn new(kind: Kind, container: Container) -> Self {
        Export {
            kind,
            container,
            model: None,
            serial: None,
            software_version: None,
            protocol: None,
            experiment: None,
            operator: None,
            acquired_at: None,
            acquired_raw: None,
            saved_raw: None,
            date_order_assumed: false,
            header: Vec::new(),
            sections: BTreeMap::new(),
            blocks: Vec::new(),
            findings: Vec::new(),
            notes: Vec::new(),
        }
    }
    /// First header value whose key matches (case-insensitive, trailing `:` ignored).
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        let want = key.trim_end_matches(':').trim();
        self.header
            .iter()
            .find(|(k, _)| k.trim_end_matches(':').trim().eq_ignore_ascii_case(want))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.trim().is_empty())
    }
    pub(crate) fn put(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.header.push((key.into(), value.into()));
    }
    /// Set `acquired_at` from a date and optional time.
    pub(crate) fn set_acquired(&mut self, date: &str, time: Option<&str>) {
        let raw = match time {
            Some(t) => format!("{} {}", date.trim(), t.trim()),
            None => date.trim().to_string(),
        };
        if let Some((iso, assumed)) = crate::datetime::combine(date, time) {
            self.acquired_at = Some(iso);
            self.date_order_assumed = assumed;
        }
        self.acquired_raw = Some(raw);
    }
    /// Finish every block: geometry, ordering.
    pub(crate) fn finish(&mut self) {
        for b in &mut self.blocks {
            b.settle_geometry();
            b.sort();
        }
    }
}

/// `0,02` → 0.02: digits, one comma, digits (a decimal comma in an otherwise dot-decimal file).
pub(crate) fn decimal_comma(s: &str) -> Option<f64> {
    let (a, b) = s.split_once(',')?;
    let a_ok = !a.is_empty()
        && a.trim_start_matches(['+', '-'])
            .chars()
            .all(|c| c.is_ascii_digit());
    (a_ok && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()))
        .then(|| format!("{a}.{b}").parse().ok())
        .flatten()
}

/// Some(trimmed) when non-empty.
pub(crate) fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The first number in a string (`Wavelengths:  450` → 450, `492nm` → 492, `360/40` → 360).
pub(crate) fn first_number(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            return s[start..i].trim_end_matches('.').parse().ok();
        }
        i += 1;
    }
    None
}

/// Every number in a string, in order.
pub(crate) fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if let Ok(v) = s[start..i].trim_end_matches('.').parse() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(first_number("Wavelengths:  450"), Some(450.0));
        assert_eq!(first_number("492nm"), Some(492.0));
        assert_eq!(numbers("360/40,460/40"), vec![360.0, 40.0, 460.0, 40.0]);
        assert_eq!(Mode::from_text("Absorbance Endpoint"), Mode::Absorbance);
        assert_eq!(Mode::from_text("Fluorescence (FI)"), Mode::Fluorescence);
        assert_eq!(Mode::from_text("Luminescence"), Mode::Luminescence);
        assert_eq!(Mode::from_text("AlphaLISA"), Mode::Alpha);
    }
}
