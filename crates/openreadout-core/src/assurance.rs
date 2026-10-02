//! Per-file assurance: does *this* file lie inside what the reader has been validated on?
//!
//! Reader confidence ([`crate::provenance::Confidence`]) is a property of a format reader. A
//! scientific answer needs more: whether the file in hand uses a format version, writer, codec
//! and layout that the reader has already read correctly on independent files. This module
//! computes that per file (`docs/assurance.md`):
//!
//! 1. Every reader declares an [`AssuranceProfile`] (in its crate's `src/assurance.rs`): a
//!    function that extracts the file's **variant features** ([`Feature`]: format version,
//!    writer, codec, sample layout, acquisition mode, ...) from the normalized [`FileInfo`],
//!    and the table of feature values seen in the development corpus ([`Validated`]),
//!    generated from the corpus results by `cargo xtask assurance-audit --write`.
//! 2. A feature value is `validated` when a development-corpus file with that value matched an
//!    independent reader (the oracle) on the outputs the feature affects, `seen` when corpus
//!    files with it were read but no independent reader confirms them, `unseen` otherwise.
//! 3. The reader also reports structures it met but did not decode ([`Undecoded`]), values it
//!    assumed instead of reading ([`Assumed`]) and vendor calibrations ([`Calibration`]).
//! 4. [`assess`] turns that into an [`Assurance`]: a level (`validated`,
//!    `partially_validated`, `unvalidated`), the reasons, and the outputs (`scope`s) that
//!    `--strict` refuses to return ([`Assurance::strict_refuses`]).
//!
//! Features with an empty scope describe the file without changing how values are decoded (the
//! writer's version, the instrument model): an unseen one lowers the level to
//! `partially_validated`. A feature with a scope changes decoding of those outputs: an unseen
//! one makes them `unvalidated`, and `--strict` refuses them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::FileInfo;
use crate::provenance::{Confidence, ProvenanceMap, Source};

/// Which outputs of a file a feature, a structure or a calibration affects.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Scope {
    /// The normalized metadata (`info`, all its views): sizes, channels, instrument, times.
    Metadata,
    /// Image planes (`check --planes`, `stats`, `export` of images, `preview`).
    Pixels,
    /// Mass spectra and scan headers (`spectra`, `analyze chromatogram`, mzML export).
    Spectra,
    /// Sampled signals (`trace`, electrophysiology, chromatograms, NMR and IR/Raman spectra).
    Traces,
    /// Tables (FCS events, plate reads, qPCR results, event and peak tables).
    Tables,
}

impl Scope {
    /// Every scope, in output order.
    pub const ALL: [Scope; 5] = [
        Scope::Metadata,
        Scope::Pixels,
        Scope::Spectra,
        Scope::Traces,
        Scope::Tables,
    ];

    /// The lowercase name used in JSON (`metadata`, `pixels`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Metadata => "metadata",
            Scope::Pixels => "pixels",
            Scope::Spectra => "spectra",
            Scope::Traces => "traces",
            Scope::Tables => "tables",
        }
    }

    /// Parse the JSON name.
    pub fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|x| x.as_str() == s)
    }
}

/// What a variant feature describes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FeatureKind {
    /// The container or format version stored in the file.
    FormatVersion,
    /// The software (or library) that wrote the file.
    Writer,
    /// The writer's version, usually reduced to major.minor.
    WriterVersion,
    /// The instrument model or family that acquired the data.
    Instrument,
    /// Compression or encoding of the stored samples.
    Codec,
    /// How samples are stored: pixel type, samples per pixel and their order, value width.
    SampleLayout,
    /// Structural layout: pyramids, mosaics, multi-file sets, directory layouts, loops.
    Layout,
    /// Acquisition mode (DDA, SRM, profile/centroid, episodic, 2-D NMR, kinetic read, ...).
    Acquisition,
    /// Dialect of a text or spreadsheet export.
    Dialect,
    /// A kind of record or block present in the file.
    Record,
    /// A normalized field the file has, among those tracked for oracle coverage
    /// ([`TRACKED_FIELDS`]): validated when development files' oracle comparisons checked it.
    Field,
    /// A rule by which a normalized value was derived rather than read
    /// (`tables[].extra.reads[].mode by detector code`): validated when development files'
    /// oracle comparisons checked the field it derives.
    Derivation,
}

impl FeatureKind {
    /// The snake_case name used in JSON and in the generated tables.
    pub fn as_str(self) -> &'static str {
        match self {
            FeatureKind::FormatVersion => "format_version",
            FeatureKind::Writer => "writer",
            FeatureKind::WriterVersion => "writer_version",
            FeatureKind::Instrument => "instrument",
            FeatureKind::Codec => "codec",
            FeatureKind::SampleLayout => "sample_layout",
            FeatureKind::Layout => "layout",
            FeatureKind::Acquisition => "acquisition",
            FeatureKind::Dialect => "dialect",
            FeatureKind::Record => "record",
            FeatureKind::Field => "field",
            FeatureKind::Derivation => "derivation",
        }
    }
}

/// One observed variant feature of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Feature {
    /// What the feature describes.
    pub kind: FeatureKind,
    /// Its value, normalized by the reader's profile (`jpeg_xr`, `3.0`, `ZEN 3.6`).
    pub value: String,
    /// The outputs whose decoding depends on it. Empty: it describes the file only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Scope>,
}

/// A structure the reader met in the file but did not decode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Undecoded {
    /// What the structure is, in the format notes' vocabulary.
    pub structure: String,
    /// Outputs that may be incomplete or wrong because of it. Empty: it is left out and the
    /// values returned do not depend on it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Scope>,
    /// What is known about it and what the reader does instead.
    pub detail: String,
}

/// A value the reader derived by a rule instead of reading it from a field that states it (a
/// read mode from a label's keywords, a unit from the instrument type).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Inferred {
    /// JSON path of the value in `info` (e.g. `tables[].extra.reads[].mode`).
    pub field: String,
    /// The rule, in words (`detector code`, `label keywords`).
    pub rule: String,
    /// What was derived from what.
    pub detail: String,
}

/// A value the reader assumed (a default, a guess from context) instead of reading it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Assumed {
    /// JSON path of the value in `info` (e.g. `traces[0].channels[1].unit`).
    pub field: String,
    /// Why it was assumed and from what.
    pub detail: String,
}

/// Whether a vendor calibration or correction stored in the file was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CalibrationStatus {
    /// Applied: returned values are calibrated as the vendor software shows them.
    Applied,
    /// Not applied: returned values differ from what the vendor software reports.
    NotApplied,
    /// Carried by the file, applied only on request or by downstream analysis (the vendor's
    /// raw view does not apply it either), e.g. FCS compensation or a flat-field profile.
    Available,
}

/// A vendor calibration or correction the file carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Calibration {
    /// What it calibrates (`m/z (MzCalibration)`, `ADC to µV`, `spillover compensation`).
    pub name: String,
    /// Applied, not applied, or available on request.
    pub status: CalibrationStatus,
    /// Outputs it applies to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Scope>,
    /// How it is (or is not) applied and how to get calibrated values.
    pub detail: String,
}

/// What a reader's profile observes in one file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Observations {
    /// Variant features.
    #[serde(default)]
    pub features: Vec<Feature>,
    /// Structures met but not decoded.
    #[serde(default)]
    pub undecoded: Vec<Undecoded>,
    /// Values assumed instead of read.
    #[serde(default)]
    pub assumed: Vec<Assumed>,
    /// Vendor calibrations and corrections.
    #[serde(default)]
    pub calibrations: Vec<Calibration>,
    /// Values derived by a rule instead of read.
    #[serde(default)]
    pub inferred: Vec<Inferred>,
}

impl Observations {
    /// A feature that changes how the outputs in `scope` are decoded. Blank values are skipped
    /// and a (kind, value) pair is recorded once, with the union of the scopes.
    pub fn feature(&mut self, kind: FeatureKind, value: impl AsRef<str>, scope: &[Scope]) {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return;
        }
        if let Some(f) = self
            .features
            .iter_mut()
            .find(|f| f.kind == kind && f.value == value)
        {
            for s in scope {
                if !f.scope.contains(s) {
                    f.scope.push(*s);
                }
            }
            f.scope.sort();
            return;
        }
        let mut scope = scope.to_vec();
        scope.sort();
        scope.dedup();
        self.features.push(Feature {
            kind,
            value: value.to_string(),
            scope,
        });
    }

    /// A feature that describes the file without changing how values are decoded.
    pub fn context(&mut self, kind: FeatureKind, value: impl AsRef<str>) {
        self.feature(kind, value, &[]);
    }

    /// A structure met but not decoded; `scope` lists the outputs it may make incomplete or
    /// wrong (empty: left out, returned values unaffected).
    pub fn undecoded(
        &mut self,
        structure: impl Into<String>,
        scope: &[Scope],
        detail: impl Into<String>,
    ) {
        let structure = structure.into();
        if self.undecoded.iter().any(|u| u.structure == structure) {
            return;
        }
        self.undecoded.push(Undecoded {
            structure,
            scope: scope.to_vec(),
            detail: detail.into(),
        });
    }

    /// A value assumed instead of read.
    pub fn assumed(&mut self, field: impl Into<String>, detail: impl Into<String>) {
        let field = field.into();
        if self.assumed.iter().any(|a| a.field == field) {
            return;
        }
        self.assumed.push(Assumed {
            field,
            detail: detail.into(),
        });
    }

    /// A value derived by `rule` instead of read (`field` its JSON path in `info`, with indices:
    /// `tables[0].extra.reads[1].mode`). The rule
    /// becomes a [`FeatureKind::Derivation`] feature `<field> by <rule>`, validated like any
    /// other feature, but only by development files whose oracle comparison checked `field`.
    pub fn derived(
        &mut self,
        field: impl Into<String>,
        rule: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let (field, rule) = (field.into(), rule.into());
        // The feature names the field without indices, so every file using the rule on that
        // field shares it; the value keeps its own path.
        self.feature(
            FeatureKind::Derivation,
            format!("{} by {rule}", generic_path(&field)),
            &[],
        );
        if self
            .inferred
            .iter()
            .any(|i| i.field == field && i.rule == rule)
        {
            return;
        }
        self.inferred.push(Inferred {
            field,
            rule,
            detail: detail.into(),
        });
    }

    /// A vendor calibration or correction.
    pub fn calibration(
        &mut self,
        name: impl Into<String>,
        status: CalibrationStatus,
        scope: &[Scope],
        detail: impl Into<String>,
    ) {
        let name = name.into();
        if self.calibrations.iter().any(|c| c.name == name) {
            return;
        }
        self.calibrations.push(Calibration {
            name,
            status,
            scope: scope.to_vec(),
            detail: detail.into(),
        });
    }

    /// Append another set of observations (a reader's own on top of its profile's).
    pub fn merge(&mut self, other: Observations) {
        for f in other.features {
            self.feature(f.kind, &f.value, &f.scope);
        }
        for u in other.undecoded {
            self.undecoded(u.structure, &u.scope, u.detail);
        }
        for a in other.assumed {
            self.assumed(a.field, a.detail);
        }
        for c in other.calibrations {
            self.calibration(c.name, c.status, &c.scope, c.detail);
        }
        for i in other.inferred {
            self.derived(i.field, i.rule, i.detail);
        }
    }

    /// True when nothing was observed.
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
            && self.undecoded.is_empty()
            && self.assumed.is_empty()
            && self.calibrations.is_empty()
            && self.inferred.is_empty()
    }
}

/// One row of a reader's validated-variant table: a feature value seen in the development
/// corpus. Generated by `cargo xtask assurance-audit --write` from `corpus/assurance/evidence.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validated {
    /// Feature kind.
    pub kind: FeatureKind,
    /// Feature value, exactly as the profile emits it.
    pub value: &'static str,
    /// Development-corpus files with this value that match an independent reader on the
    /// outputs the feature affects.
    pub files: u32,
    /// Distinct depositors (source records) among those files.
    pub sources: u32,
    /// Development-corpus files with this value that were read, with or without an oracle.
    pub seen: u32,
}

/// A [`Validated`] row (the generated tables use it to stay one line per value).
pub const fn row(
    kind: FeatureKind,
    value: &'static str,
    files: u32,
    sources: u32,
    seen: u32,
) -> Validated {
    Validated {
        kind,
        value,
        files,
        sources,
        seen,
    }
}

/// Where a reader's understanding of its format comes from (one input to the confidence
/// rubric, `docs/assurance.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Basis {
    /// An open, published specification (FCS, mzML, NetCDF-ANDI, OME, NWB, JCAMP-DX, MRC).
    OpenSpec,
    /// The vendor's own published documentation or open-source code.
    VendorDocs,
    /// Documentation of permissively licensed community readers plus corpus files.
    PriorArt,
    /// Reverse-engineered from corpus files alone.
    ReverseEngineered,
}

impl Basis {
    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Basis::OpenSpec => "open_spec",
            Basis::VendorDocs => "vendor_docs",
            Basis::PriorArt => "prior_art",
            Basis::ReverseEngineered => "reverse_engineered",
        }
    }
}

/// A reader's assurance declaration (its crate's `src/assurance.rs`).
#[derive(Debug, Clone, Copy)]
pub struct AssuranceProfile {
    /// Format id the profile belongs to.
    pub format_id: &'static str,
    /// Extract the variant features (and undecoded structures, assumptions, calibrations
    /// visible in the normalized output) from `info`.
    pub observe: fn(&FileInfo) -> Observations,
    /// Feature values seen in the development corpus (generated).
    pub validated: &'static [Validated],
    /// Reader confidence from the evidence rubric (generated; `docs/assurance.md`).
    pub confidence: Confidence,
    /// Where the reader's knowledge of the format comes from.
    pub basis: Basis,
}

impl AssuranceProfile {
    /// The table row for a feature, if the corpus has seen its value.
    pub fn lookup(&self, f: &Feature) -> Option<&'static Validated> {
        self.validated
            .iter()
            .find(|v| v.kind == f.kind && v.value == f.value)
    }
}

static DEFAULT_STRICT: AtomicBool = AtomicBool::new(false);

/// Make every registry created after this call strict (`--strict` sets it for the whole process,
/// so batch commands and the MCP server's tools inherit it).
pub fn set_default_strict(on: bool) {
    DEFAULT_STRICT.store(on, Ordering::Relaxed);
}

/// Whether new registries start strict: [`set_default_strict`], or the environment variable
/// `OPENREADOUT_STRICT` set to anything but empty, `0` or `false`.
pub fn default_strict() -> bool {
    static ENV: OnceLock<bool> = OnceLock::new();
    DEFAULT_STRICT.load(Ordering::Relaxed)
        || *ENV.get_or_init(|| {
            std::env::var("OPENREADOUT_STRICT")
                .is_ok_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
        })
}

/// Profiles by format id, filled as readers are registered ([`crate::Registry::register`]).
fn profiles() -> &'static RwLock<BTreeMap<&'static str, &'static AssuranceProfile>> {
    static P: OnceLock<RwLock<BTreeMap<&'static str, &'static AssuranceProfile>>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Make `profile` known under its format id (done by [`crate::Registry::register`]).
pub fn register_profile(profile: &'static AssuranceProfile) {
    if let Ok(mut m) = profiles().write() {
        m.insert(profile.format_id, profile);
    }
}

/// The registered profile of a format id.
pub fn profile(format_id: &str) -> Option<&'static AssuranceProfile> {
    profiles().read().ok()?.get(format_id).copied()
}

/// How far a file lies inside what its reader has been validated on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssuranceLevel {
    /// Every variant feature was read correctly on development files confirmed by an
    /// independent reader; nothing undecoded, assumed or left uncalibrated.
    Validated,
    /// Values are decoded along validated paths, but something is less certain: a descriptive
    /// feature (writer version, instrument) never seen, a feature seen only without an
    /// independent reader, a structure left out, or a value assumed.
    PartiallyValidated,
    /// Some output depends on a variant never validated (or on an undecoded structure or an
    /// unapplied calibration): its values may be wrong. `strict_refuses` lists those outputs.
    Unvalidated,
}

impl AssuranceLevel {
    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            AssuranceLevel::Validated => "validated",
            AssuranceLevel::PartiallyValidated => "partially_validated",
            AssuranceLevel::Unvalidated => "unvalidated",
        }
    }
}

/// How the corpus covers one feature value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FeatureStatus {
    /// Development files with this value match an independent reader.
    Validated,
    /// Development files with this value were read, but no independent reader confirms them.
    Seen,
    /// No development file has this value.
    Unseen,
}

/// One feature of the file's fingerprint, with the corpus evidence for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VariantFeature {
    /// What the feature describes.
    pub kind: FeatureKind,
    /// Its value.
    pub value: String,
    /// Outputs whose decoding depends on it (empty: descriptive only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Scope>,
    /// `validated`, `seen` or `unseen`.
    pub status: FeatureStatus,
    /// Development-corpus files with this value confirmed by an independent reader.
    pub corpus_files: u32,
    /// Distinct depositors among them.
    pub sources: u32,
}

/// Fields whose meaning was inferred (reverse-engineered from corpus files), not read from a
/// specification: the `inferred` entries of the reader's provenance map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InferredFields {
    /// How many normalized fields have inferred meaning.
    pub count: u32,
    /// How many normalized fields carry provenance at all.
    pub of: u32,
    /// Up to 12 of their JSON paths (`info --view full` → `provenance` has all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

/// A value derived by a rule, with the corpus evidence for the rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InferredValue {
    /// JSON path of the value in `info`.
    pub field: String,
    /// The rule that derived it.
    pub rule: String,
    /// `validated` when development files' oracles confirmed values derived by this rule.
    pub status: FeatureStatus,
    /// What was derived from what.
    pub detail: String,
}

/// A field `--strict` withholds: it is present, but its value was assumed, derived by a rule no
/// independent reader confirmed, or never compared with an independent reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Withheld {
    /// JSON path pattern in `info` (`[]` matches any index).
    pub field: String,
    /// Why it is not validated.
    pub reason: String,
}

/// The per-file assurance block of `info` (every view) and `check`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Assurance {
    /// `validated`, `partially_validated` or `unvalidated`.
    pub level: AssuranceLevel,
    /// One line for people and agents.
    pub summary: String,
    /// The variant fingerprint: `format_id` then `kind=value` for each feature, sorted.
    pub fingerprint: String,
    /// Each feature with its corpus evidence.
    pub variant: Vec<VariantFeature>,
    /// Why the level is not `validated`, one reason per line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// Structures met but not decoded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undecoded: Vec<Undecoded>,
    /// Values assumed instead of read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumed: Vec<Assumed>,
    /// Vendor calibrations and corrections the file carries, and whether they were applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calibrations: Vec<Calibration>,
    /// Normalized fields whose meaning is inferred rather than specified.
    #[serde(default, skip_serializing_if = "is_default_inferred")]
    pub inferred_fields: InferredFields,
    /// Values derived by a rule instead of read, with the evidence for each rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inferred: Vec<InferredValue>,
    /// Outputs `--strict` (MCP `strict: true`) refuses to return for this file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strict_refuses: Vec<Scope>,
    /// Fields `--strict` withholds (returned as null; asking for one with `--only` exits 6)
    /// while the rest of the output is returned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strict_withholds: Vec<Withheld>,
    /// The reader's overall confidence (evidence rubric), for context.
    pub reader_confidence: Confidence,
}

fn is_default_inferred(v: &InferredFields) -> bool {
    v.count == 0 && v.of == 0
}

impl Assurance {
    /// True when `--strict` refuses output `scope`.
    pub fn refuses(&self, scope: Scope) -> bool {
        self.strict_refuses.contains(&scope)
    }

    /// The `Unsupported` error `--strict` returns for `scope` (exit 6), if it refuses it.
    pub fn strict_error(&self, format_id: &str, scope: Scope) -> Option<Error> {
        if !self.refuses(scope) {
            return None;
        }
        // Reasons about scoped problems start with `[scope,scope] `; keep those naming `scope`.
        let why: Vec<&str> = self
            .reasons
            .iter()
            .filter_map(|r| {
                let (scopes, text) = r.strip_prefix('[')?.split_once("] ")?;
                scopes
                    .split(',')
                    .any(|s| s == scope.as_str())
                    .then_some(text)
            })
            .collect();
        Some(Error::Unsupported {
            format: "strict",
            feature: format!(
                "returning {} from a {format_id} file outside the validated variants ({})",
                scope.as_str(),
                if why.is_empty() {
                    self.summary.clone()
                } else {
                    why.join("; ")
                }
            ),
            hint: Some(format!(
                "--strict (MCP strict: true) refuses {} that no independent reader has confirmed for this kind of file. Rerun without --strict to read them anyway and treat them as unverified (`info --json` → assurance says why), or check them against another reader.",
                scope.as_str()
            )),
        })
    }
}

/// Assess one file: its reader's profile applied to `info`, plus what the reader reports
/// itself (`extra`) and its provenance map.
pub fn assess(info: &FileInfo, extra: Observations, provenance: &ProvenanceMap) -> Assurance {
    let experiment = crate::experiment::derive(info, provenance);
    assess_full(info, extra, provenance, &experiment)
}

/// [`assess`] with the file's experiment (derived, plus the reader's own facts).
fn assess_full(
    info: &FileInfo,
    extra: Observations,
    provenance: &ProvenanceMap,
    experiment: &crate::experiment::Experiment,
) -> Assurance {
    let format_id = info.format.id.as_str();
    let mut obs = match profile(format_id) {
        Some(p) => (p.observe)(info),
        None => Observations::default(),
    };
    obs.merge(extra);
    obs.merge(tracked_fields(info, experiment));
    let mut a = match profile(format_id) {
        Some(p) => assess_with(p, format_id, obs, provenance),
        None => unprofiled(info, obs, provenance),
    };
    // A withheld experiment value is withheld where it was read from too.
    let mut sources = Vec::new();
    for w in &a.strict_withholds {
        if let Some(key) = w.field.strip_prefix("experiment.")
            && let Some(src) = experiment_source(experiment, key)
            && !a.strict_withholds.iter().any(|x| x.field == src)
            && !sources.iter().any(|x: &Withheld| x.field == src)
        {
            sources.push(Withheld {
                field: src,
                reason: w.reason.clone(),
            });
        }
    }
    a.strict_withholds.extend(sources);
    // ... and an experiment value read from a withheld field is withheld with it.
    let mut derived = Vec::new();
    for (key, origin) in &experiment.provenance {
        let pointer = path_pointer(&origin.from);
        let field = format!("experiment.{key}");
        if let Some(w) = a
            .strict_withholds
            .iter()
            .find(|w| pattern_covers(&w.field, &pointer))
            && !a.strict_withholds.iter().any(|x| x.field == field)
            && !derived.iter().any(|x: &Withheld| x.field == field)
        {
            derived.push(Withheld {
                field,
                reason: w.reason.clone(),
            });
        }
    }
    a.strict_withholds.extend(derived);
    a
}

/// `tables[0].extra.acquired_at` → `/tables/0/extra/acquired_at`.
fn path_pointer(path: &str) -> String {
    let mut out = String::from("/");
    out.push_str(
        &path
            .replace("][", "/")
            .replace(['[', '.'], "/")
            .replace(']', ""),
    );
    out
}

/// Normalized fields tracked for oracle coverage: the values questions ask about most whose
/// errors would not show in decoded values (`docs/assurance.md` § Field coverage). Each is a
/// [`FeatureKind::Field`] feature when the file has it; a development file validates it only
/// when its oracle comparison checked that field.
pub const TRACKED_FIELDS: &[&str] = &[
    "experiment.acquisition.started_at",
    "experiment.instrument.model",
    "tables[].extra.reads[].mode",
];

/// The tracked fields `info` (and its experiment) holds, as `Field` features, with the paths
/// they were taken from.
fn tracked_fields(info: &FileInfo, experiment: &crate::experiment::Experiment) -> Observations {
    let mut o = Observations::default();
    // A value read from a field whose meaning a published specification or the vendor's own
    // documentation gives is read from a validated location; any other needs an oracle.
    let documented = |key: &str| {
        experiment
            .provenance
            .get(key)
            .is_some_and(|origin| matches!(origin.source, Source::Spec | Source::VendorImpl))
    };
    if experiment
        .acquisition
        .as_ref()
        .is_some_and(|a| a.started_at.is_some())
        && !documented("acquisition.started_at")
    {
        o.context(FeatureKind::Field, TRACKED_FIELDS[0]);
    }
    if experiment
        .instrument
        .as_ref()
        .is_some_and(|i| i.model.is_some())
        && !documented("instrument.model")
    {
        o.context(FeatureKind::Field, TRACKED_FIELDS[1]);
    }
    let measured_mode = info.tables.iter().any(|t| {
        t.extra
            .get("reads")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|reads| {
                reads.iter().any(|r| {
                    r.get("calculated").and_then(serde_json::Value::as_bool) != Some(true)
                        && r.get("mode").is_some()
                })
            })
    });
    if measured_mode {
        o.context(FeatureKind::Field, TRACKED_FIELDS[2]);
    }
    o
}

/// Where an experiment value came from (its provenance `from` path, with indices as `[]`).
fn experiment_source(experiment: &crate::experiment::Experiment, key: &str) -> Option<String> {
    experiment
        .provenance
        .get(key)
        .map(|o| generic_path(&o.from))
        .filter(|p| !p.is_empty() && !p.contains('/'))
}

/// `traces[0].extra.acquired_at` → `traces[].extra.acquired_at`.
fn generic_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut in_index = false;
    for c in p.chars() {
        match c {
            '[' => {
                in_index = true;
                out.push_str("[]");
            }
            ']' => in_index = false,
            _ if in_index => {}
            _ => out.push(c),
        }
    }
    out
}

fn inferred(provenance: &ProvenanceMap) -> InferredFields {
    let of = u32::try_from(provenance.len()).unwrap_or(u32::MAX);
    let paths: Vec<&String> = provenance
        .iter()
        .filter(|(_, s)| **s == Source::Inferred)
        .map(|(k, _)| k)
        .collect();
    InferredFields {
        count: u32::try_from(paths.len()).unwrap_or(u32::MAX),
        of,
        examples: paths.iter().take(12).map(|s| (*s).clone()).collect(),
    }
}

fn fingerprint(format_id: &str, features: &[Feature]) -> String {
    // Field features describe which values the file holds, not its variant.
    let mut parts: Vec<String> = features
        .iter()
        .filter(|f| f.kind != FeatureKind::Field)
        .map(|f| format!("{}={}", f.kind.as_str(), f.value))
        .collect();
    parts.sort();
    parts.dedup();
    let mut s = format_id.to_string();
    for p in parts {
        s.push('|');
        s.push_str(&p);
    }
    s
}

fn scopes_text(scope: &[Scope]) -> String {
    scope
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn unprofiled(info: &FileInfo, obs: Observations, provenance: &ProvenanceMap) -> Assurance {
    let format_id = info.format.id.as_str();
    let variant = obs
        .features
        .iter()
        .map(|f| VariantFeature {
            kind: f.kind,
            value: f.value.clone(),
            scope: f.scope.clone(),
            status: FeatureStatus::Unseen,
            corpus_files: 0,
            sources: 0,
        })
        .collect();
    let inferred_values = obs
        .inferred
        .iter()
        .map(|i| InferredValue {
            field: i.field.clone(),
            rule: i.rule.clone(),
            status: FeatureStatus::Unseen,
            detail: i.detail.clone(),
        })
        .collect();
    Assurance {
        level: AssuranceLevel::Unvalidated,
        summary: format!(
            "unvalidated: the {format_id} reader declares no validated variants, so nothing says this file lies inside what it was tested on"
        ),
        fingerprint: fingerprint(format_id, &obs.features),
        variant,
        reasons: vec![format!(
            "the {format_id} reader has no assurance profile (no validated variant set)"
        )],
        undecoded: obs.undecoded,
        assumed: obs.assumed,
        calibrations: obs.calibrations,
        inferred_fields: inferred(provenance),
        inferred: inferred_values,
        strict_refuses: Scope::ALL.to_vec(),
        strict_withholds: Vec::new(),
        reader_confidence: info.format.confidence,
    }
}

/// A version value split into its family (the text before the first digit, lower case) and up
/// to three numeric components: `ZEN 3.6.095` → (`zen`, [3, 6, 95]), `1.83` → (``, [1, 83]).
pub fn version_key(value: &str) -> Option<(String, Vec<u64>)> {
    let start = value.find(|c: char| c.is_ascii_digit())?;
    let family = value[..start].trim().to_ascii_lowercase();
    let nums: Vec<u64> = value[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(3)
        .filter_map(|p| p.parse().ok())
        .collect();
    (!nums.is_empty()).then_some((family, nums))
}

/// For an unseen format or writer version: the closest validated versions of the same family
/// and major version below and above it, when both exist.
fn bracketing(profile: &AssuranceProfile, f: &Feature) -> Option<(&'static str, &'static str)> {
    if !matches!(
        f.kind,
        FeatureKind::FormatVersion | FeatureKind::WriterVersion
    ) {
        return None;
    }
    let (family, v) = version_key(&f.value)?;
    let mut lo: Option<(Vec<u64>, &'static str)> = None;
    let mut hi: Option<(Vec<u64>, &'static str)> = None;
    for row in profile
        .validated
        .iter()
        .filter(|r| r.kind == f.kind && r.files > 0)
    {
        let Some((fam, w)) = version_key(row.value) else {
            continue;
        };
        if fam != family || w.first() != v.first() {
            continue;
        }
        if w < v && lo.as_ref().is_none_or(|(b, _)| w > *b) {
            lo = Some((w, row.value));
        } else if w > v && hi.as_ref().is_none_or(|(b, _)| w < *b) {
            hi = Some((w, row.value));
        }
    }
    Some((lo?.1, hi?.1))
}

/// [`assess`] with an explicit profile (tests, and readers assessing before registration).
pub fn assess_with(
    profile: &AssuranceProfile,
    format_id: &str,
    obs: Observations,
    provenance: &ProvenanceMap,
) -> Assurance {
    let mut reasons: Vec<String> = Vec::new();
    let mut refuse: BTreeSet<Scope> = BTreeSet::new();
    let mut partial = false;
    let mut variant = Vec::with_capacity(obs.features.len());
    let mut validated_files = 0u32;
    let mut validated_sources = 0u32;
    let mut withholds: Vec<Withheld> = Vec::new();
    let mut withhold = |field: &str, reason: String| {
        if !withholds.iter().any(|w| w.field == field) {
            withholds.push(Withheld {
                field: field.to_string(),
                reason,
            });
        }
    };
    for f in &obs.features {
        let row = profile.lookup(f);
        let status = match row {
            Some(v) if v.files > 0 => FeatureStatus::Validated,
            Some(_) => FeatureStatus::Seen,
            None => FeatureStatus::Unseen,
        };
        let (files, sources) = row.map_or((0, 0), |v| (v.files, v.sources));
        if status == FeatureStatus::Validated {
            validated_files = validated_files.max(files);
            validated_sources = validated_sources.max(sources);
        }
        let label = format!("{} {}", f.kind.as_str().replace('_', " "), f.value);
        // Fields and derivation rules concern one value, not how outputs are decoded: when no
        // independent reader confirmed them, `--strict` withholds that value alone.
        let field_kind = matches!(f.kind, FeatureKind::Field | FeatureKind::Derivation);
        match (status, f.scope.is_empty()) {
            (FeatureStatus::Validated, _) => {}
            (_, _) if field_kind => {
                // A value derived by an unconfirmed rule is like an assumed one (partial); a
                // field no oracle compared is read along a validated path, so only that value
                // is withheld and the level is unchanged.
                if f.kind == FeatureKind::Derivation {
                    partial = true;
                }
                let field = match f.kind {
                    FeatureKind::Derivation => f
                        .value
                        .split_once(" by ")
                        .map_or(f.value.as_str(), |(field, _)| field),
                    _ => f.value.as_str(),
                };
                let why = match f.kind {
                    FeatureKind::Derivation => format!(
                        "derived by a rule ({}) that no independent reader has confirmed on a development file",
                        f.value.split_once(" by ").map_or("", |(_, r)| r)
                    ),
                    _ => "never compared with an independent reader on a development file of this format".to_string(),
                };
                reasons.push(format!("{field}: {why}; --strict withholds it"));
                if f.kind == FeatureKind::Field {
                    withhold(field, why);
                }
            }
            (FeatureStatus::Unseen, false) => {
                if let Some((lo, hi)) = bracketing(profile, f) {
                    // A version between two validated versions of the same family and major
                    // version: the format is taken to be read the same way (partial, not refused).
                    partial = true;
                    reasons.push(format!(
                        "{label}: not in the development corpus, but between the validated versions {lo} and {hi}"
                    ));
                } else {
                    refuse.extend(f.scope.iter().copied());
                    reasons.push(format!(
                        "[{}] {label}: never seen in a development-corpus file, so how it is decoded is unvalidated",
                        scopes_text(&f.scope)
                    ));
                }
            }
            (FeatureStatus::Unseen, true) => {
                partial = true;
                reasons.push(format!(
                    "{label}: not in the development corpus (descriptive; decoding does not depend on it)"
                ));
            }
            (FeatureStatus::Seen, _) => {
                partial = true;
                reasons.push(format!(
                    "{label}: read in {} development-corpus file(s), none confirmed by an independent reader",
                    row.map_or(0, |v| v.seen)
                ));
            }
        }
        variant.push(VariantFeature {
            kind: f.kind,
            value: f.value.clone(),
            scope: f.scope.clone(),
            status,
            corpus_files: files,
            sources,
        });
    }
    for u in &obs.undecoded {
        if u.scope.is_empty() {
            partial = true;
            reasons.push(format!(
                "{}: present but not decoded (left out)",
                u.structure
            ));
        } else {
            refuse.extend(u.scope.iter().copied());
            reasons.push(format!(
                "[{}] {}: present but not decoded; these outputs may be incomplete or wrong",
                scopes_text(&u.scope),
                u.structure
            ));
        }
    }
    for c in &obs.calibrations {
        if c.status == CalibrationStatus::NotApplied {
            refuse.extend(c.scope.iter().copied());
            reasons.push(format!(
                "[{}] {}: vendor calibration not applied; values differ from the vendor software's",
                scopes_text(&c.scope),
                c.name
            ));
        }
    }
    for a in &obs.assumed {
        withhold(&a.field, format!("assumed rather than read: {}", a.detail));
    }
    if !obs.assumed.is_empty() {
        partial = true;
        reasons.push(format!(
            "{} value(s) assumed rather than read: {}",
            obs.assumed.len(),
            obs.assumed
                .iter()
                .map(|a| a.field.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if obs.features.iter().all(|f| f.kind == FeatureKind::Field) {
        partial = true;
        reasons.push("the reader's profile found no variant features to check".into());
    }
    let inferred_values: Vec<InferredValue> = obs
        .inferred
        .iter()
        .map(|i| InferredValue {
            field: i.field.clone(),
            rule: i.rule.clone(),
            status: variant
                .iter()
                .find(|v| {
                    v.kind == FeatureKind::Derivation
                        && v.value == format!("{} by {}", generic_path(&i.field), i.rule)
                })
                .map_or(FeatureStatus::Unseen, |v| v.status),
            detail: i.detail.clone(),
        })
        .collect();
    // A value derived by a rule no independent reader confirmed is withheld (that value only).
    for i in inferred_values
        .iter()
        .filter(|i| i.status != FeatureStatus::Validated)
    {
        withhold(
            &i.field,
            format!(
                "derived by a rule ({}) that no independent reader has confirmed on a development file",
                i.rule
            ),
        );
    }
    let level = if !refuse.is_empty() {
        AssuranceLevel::Unvalidated
    } else if partial {
        AssuranceLevel::PartiallyValidated
    } else {
        AssuranceLevel::Validated
    };
    let strict_refuses: Vec<Scope> = refuse.into_iter().collect();
    let summary = match level {
        AssuranceLevel::Validated => format!(
            "validated: every variant feature of this file was read correctly in development-corpus files confirmed by an independent reader (up to {validated_files} files from {validated_sources} sources)"
        ),
        AssuranceLevel::PartiallyValidated => format!(
            "partially validated: decoded along validated paths, but {}",
            reasons
                .first()
                .map_or("some evidence is missing", String::as_str)
        ),
        AssuranceLevel::Unvalidated => format!(
            "UNVALIDATED for {}: {}; values there may be wrong (--strict refuses them)",
            scopes_text(&strict_refuses),
            reasons
                .iter()
                .find(|r| r.starts_with('['))
                .map_or("unvalidated variant", |r| r
                    .split_once("] ")
                    .map_or(r.as_str(), |(_, t)| t))
        ),
    };
    Assurance {
        level,
        summary,
        fingerprint: fingerprint(format_id, &obs.features),
        variant,
        reasons,
        undecoded: obs.undecoded,
        assumed: obs.assumed,
        calibrations: obs.calibrations,
        inferred_fields: inferred(provenance),
        inferred: inferred_values,
        strict_refuses,
        strict_withholds: withholds,
        reader_confidence: profile.confidence,
    }
}

/// Assess an open dataset (its `info`, its own observations and its provenance map).
pub fn assess_dataset(ds: &dyn crate::Dataset, info: &FileInfo) -> Assurance {
    let experiment = crate::experiment::of_dataset(ds, info);
    assess_full(
        info,
        ds.assurance_observations(),
        &ds.provenance(),
        &experiment,
    )
}

/// `--strict`: fail with exit 6 when the file's assurance refuses `scope`.
pub fn gate(ds: &dyn crate::Dataset, info: &FileInfo, scope: Scope) -> Result<()> {
    let a = assess_dataset(ds, info);
    match a.strict_error(&info.format.id, scope) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ---------- withholding (`--strict`) ----------

/// One segment of a field pattern: an object key, then which array elements (if an array).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Elements {
    /// Not an array.
    No,
    /// `[]`: every element.
    All,
    /// `[n]`: element n.
    At(usize),
}

/// A field pattern's segments (`tables[0].extra.reads[].mode`).
fn pattern_segments(field: &str) -> Vec<(&str, Elements)> {
    field
        .split('.')
        .map(|s| match s.split_once('[') {
            Some((k, rest)) => {
                let inner = rest.trim_end_matches(']');
                if inner.is_empty() {
                    (k, Elements::All)
                } else {
                    (k, inner.parse().map_or(Elements::All, Elements::At))
                }
            }
            None => (s, Elements::No),
        })
        .collect()
}

/// Does JSON pointer `pointer` (`/tables/0/extra/reads/1/mode`) address the value a field
/// pattern (`tables[].extra.reads[].mode`, `tables[0].extra.reads[1].mode`) names, or something
/// inside it?
pub(crate) fn pattern_covers(field: &str, pointer: &str) -> bool {
    let segs: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
    let mut i = 0;
    for (key, elements) in pattern_segments(field) {
        if segs.get(i) != Some(&key) {
            return false;
        }
        i += 1;
        match elements {
            Elements::No => {}
            Elements::All => match segs.get(i) {
                Some(s) if *s == "*" || s.chars().all(|c| c.is_ascii_digit()) => i += 1,
                _ => return false,
            },
            Elements::At(n) => match segs.get(i) {
                Some(s) if *s == "*" || s.parse::<usize>() == Ok(n) => i += 1,
                _ => return false,
            },
        }
    }
    true
}

/// The withheld field a pointer asks for, if any.
pub fn withheld_for<'a>(a: &'a Assurance, pointer: &str) -> Option<&'a Withheld> {
    a.strict_withholds
        .iter()
        .find(|w| pattern_covers(&w.field, pointer))
}

/// Replace every value `a.strict_withholds` names in `v` (the JSON of `info`) with null;
/// returns how many values were withheld.
pub(crate) fn withhold(v: &mut serde_json::Value, a: &Assurance) -> usize {
    fn null(x: &mut serde_json::Value) -> usize {
        if x.is_null() {
            0
        } else {
            *x = serde_json::Value::Null;
            1
        }
    }
    fn go(v: &mut serde_json::Value, segs: &[(&str, Elements)]) -> usize {
        let Some(((key, elements), rest)) = segs.split_first() else {
            return 0;
        };
        let Some(child) = v.as_object_mut().and_then(|o| o.get_mut(*key)) else {
            return 0;
        };
        let at = |x: &mut serde_json::Value| {
            if rest.is_empty() {
                null(x)
            } else {
                go(x, rest)
            }
        };
        match elements {
            Elements::No => at(child),
            Elements::All => child
                .as_array_mut()
                .map_or(0, |items| items.iter_mut().map(at).sum()),
            Elements::At(n) => child
                .as_array_mut()
                .and_then(|items| items.get_mut(*n))
                .map_or(0, at),
        }
    }
    a.strict_withholds
        .iter()
        .map(|w| go(v, &pattern_segments(&w.field)))
        .sum()
}

// ---------- helpers for profiles ----------

/// The first `n` dot-separated numeric components of a version string (`3.6.095.00000` →
/// `3.6` for n = 2; `7,0,0,268` → `7.0`; `5.20.02 (Build 1453)` → `5.20`; `2.4 SP1` → `2.4`).
/// Leading text is skipped (`v1.2` → `1.2`). `None` when there is no number.
pub fn version_prefix(s: &str, n: usize) -> Option<String> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let rest = &s[start..];
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split(['.', ',']) {
        let num: &str = p.find(|c: char| !c.is_ascii_digit()).map_or(p, |i| &p[..i]);
        if num.is_empty() {
            break;
        }
        parts.push(num);
        if parts.len() == n || num.len() != p.len() {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    // Drop leading zeros of every component but keep "0".
    Some(
        parts
            .iter()
            .map(|p| {
                let t = p.trim_start_matches('0');
                if t.is_empty() { "0" } else { t }
            })
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// A string value of `extra[key]`.
pub fn extra_str<'a>(extra: &'a BTreeMap<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    extra.get(key).and_then(serde_json::Value::as_str)
}

/// `extra[key]` as text: strings as they are, numbers and booleans formatted, arrays of those
/// joined with `+`.
pub fn extra_text(extra: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<String> {
    value_text(extra.get(key)?)
}

/// The elements of `extra[key]` as text (each element of an array, or the single value).
pub fn extra_values_in(extra: &BTreeMap<String, serde_json::Value>, key: &str) -> Vec<String> {
    match extra.get(key) {
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(value_text).collect(),
        Some(v) => value_text(v).into_iter().collect(),
        None => Vec::new(),
    }
}

/// A JSON scalar (or array of scalars, joined with `+`) as text.
pub fn value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Array(a) => {
            let parts: Vec<String> = a.iter().filter_map(value_text).collect();
            (!parts.is_empty()).then(|| parts.join("+"))
        }
        _ => None,
    }
}

/// Features every image reader shares: the format version, and per image the stored sample
/// layout (`uint16`, `uint8x3`, ...), all affecting pixels.
pub fn image_basics(obs: &mut Observations, info: &FileInfo) {
    if let Some(v) = &info.format_version {
        obs.feature(
            FeatureKind::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Pixels],
        );
    }
    for im in &info.images {
        let layout = if im.samples_per_pixel > 1 {
            format!("{}x{}", im.pixel_type.ome_name(), im.samples_per_pixel)
        } else {
            im.pixel_type.ome_name().to_string()
        };
        obs.feature(FeatureKind::SampleLayout, layout, &[Scope::Pixels]);
    }
}

/// The writer name and major.minor version of the first image or spectra run that records them
/// (descriptive).
pub fn writer_context(obs: &mut Observations, info: &FileInfo) {
    let inst = info
        .images
        .iter()
        .find_map(|i| i.instrument.as_ref())
        .or_else(|| info.spectra.iter().find_map(|s| s.instrument.as_ref()));
    if let Some(i) = inst
        && let Some(sw) = &i.software
    {
        obs.context(FeatureKind::Writer, sw);
        if let Some(v) = i
            .software_version
            .as_deref()
            .and_then(|v| version_prefix(v, 2))
        {
            obs.context(FeatureKind::WriterVersion, format!("{sw} {v}"));
        }
    }
}

/// The instrument model of the first image or spectra run that records one (descriptive).
pub fn instrument_context(obs: &mut Observations, info: &FileInfo) {
    let model = info
        .images
        .iter()
        .find_map(|i| i.instrument.as_ref().and_then(|x| x.model.clone()))
        .or_else(|| {
            info.spectra
                .iter()
                .find_map(|s| s.instrument.as_ref().and_then(|x| x.model.clone()))
        });
    if let Some(m) = model {
        obs.context(FeatureKind::Instrument, m);
    }
}

/// The `extra` maps of every image, table, spectra run and trace, in that order.
pub fn extras(info: &FileInfo) -> impl Iterator<Item = &BTreeMap<String, serde_json::Value>> {
    info.images
        .iter()
        .map(|i| &i.extra)
        .chain(info.tables.iter().map(|t| &t.extra))
        .chain(info.spectra.iter().map(|s| &s.extra))
        .chain(info.traces.iter().map(|t| &t.extra))
}

/// Every distinct text value of `extra[key]` across images, tables, spectra and traces, in
/// first-seen order.
pub fn extra_values(info: &FileInfo, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in extras(info) {
        if let Some(v) = extra_text(e, key)
            && !out.contains(&v)
        {
            out.push(v);
        }
    }
    out
}

/// True when a note starts with `prefix` (notes are the readers' stable, documented lines).
pub fn has_note(info: &FileInfo, prefix: &str) -> bool {
    info.notes.iter().any(|n| n.starts_with(prefix))
}

/// The first note containing `needle`.
pub fn note_with<'a>(info: &'a FileInfo, needle: &str) -> Option<&'a str> {
    info.notes
        .iter()
        .find(|n| n.contains(needle))
        .map(String::as_str)
}

/// A writer's name without its version: the words before the first one that starts with a
/// digit (or `v` + digit): `MetaMorph 6.2.3.733` → `MetaMorph`, `Aperio Image Library v11.2.1`
/// → `Aperio Image Library`, `BD FACSDiva Software Version 6.2` → `BD FACSDiva Software`.
/// A trailing `Version`/`version` word is dropped too.
pub fn writer_name_only(s: &str) -> String {
    let mut words: Vec<&str> = Vec::new();
    for w in s.split_whitespace() {
        let t = w.trim_start_matches(['v', 'V']);
        if t.starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        words.push(w);
    }
    while words
        .last()
        .is_some_and(|w| w.eq_ignore_ascii_case("version") || *w == "-" || *w == ",")
    {
        words.pop();
    }
    if words.is_empty() {
        s.trim().to_string()
    } else {
        words.join(" ")
    }
}

/// A value reduced to a stable token: lowercase, runs of anything but letters and digits as
/// one `_`, trimmed (`JPEG XR` → `jpeg_xr`, `Q Exactive HF Orbitrap` → `q_exactive_hf_orbitrap`).
pub fn token(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut sep = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if sep && !out.is_empty() {
                out.push('_');
            }
            sep = false;
            out.extend(c.to_lowercase());
        } else {
            sep = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(validated: &'static [Validated]) -> AssuranceProfile {
        AssuranceProfile {
            format_id: "test",
            observe: |_| Observations::default(),
            validated,
            confidence: Confidence::Medium,
            basis: Basis::ReverseEngineered,
        }
    }

    const ROWS: &[Validated] = &[
        Validated {
            kind: FeatureKind::FormatVersion,
            value: "1.0",
            files: 12,
            sources: 3,
            seen: 12,
        },
        Validated {
            kind: FeatureKind::Codec,
            value: "zstd",
            files: 0,
            sources: 0,
            seen: 2,
        },
    ];

    fn obs(features: &[(FeatureKind, &str, &[Scope])]) -> Observations {
        let mut o = Observations::default();
        for (k, v, s) in features {
            o.feature(*k, v, s);
        }
        o
    }

    const FIELD_ROWS: &[Validated] = &[
        Validated {
            kind: FeatureKind::FormatVersion,
            value: "1.0",
            files: 12,
            sources: 3,
            seen: 12,
        },
        Validated {
            kind: FeatureKind::Derivation,
            value: "tables[].extra.reads[].mode by detector code",
            files: 4,
            sources: 2,
            seen: 4,
        },
        Validated {
            kind: FeatureKind::Field,
            value: "tables[].extra.reads[].mode",
            files: 9,
            sources: 3,
            seen: 9,
        },
    ];

    #[test]
    fn derived_values_are_withheld_only_when_their_rule_is_unconfirmed() {
        let p = profile_with(FIELD_ROWS);
        let mut o = obs(&[
            (FeatureKind::FormatVersion, "1.0", &[Scope::Tables]),
            (FeatureKind::Field, "tables[].extra.reads[].mode", &[]),
        ]);
        o.derived(
            "tables[0].extra.reads[0].mode",
            "detector code",
            "lum from De=USLum",
        );
        o.derived(
            "tables[0].extra.reads[1].mode",
            "label keywords",
            "lum from `US LUM`",
        );
        let a = assess_with(&p, "test", o, &ProvenanceMap::new());
        // The confirmed rule's value is returned; the other one alone is withheld. Nothing is
        // refused as a whole.
        assert_eq!(a.level, AssuranceLevel::PartiallyValidated);
        assert!(a.strict_refuses.is_empty());
        assert_eq!(a.inferred.len(), 2);
        assert_eq!(a.inferred[0].status, FeatureStatus::Validated);
        assert_eq!(a.inferred[1].status, FeatureStatus::Unseen);
        let fields: Vec<&str> = a
            .strict_withholds
            .iter()
            .map(|w| w.field.as_str())
            .collect();
        assert_eq!(fields, vec!["tables[0].extra.reads[1].mode"]);
        assert!(withheld_for(&a, "/tables/0/extra/reads/1/mode").is_some());
        assert!(withheld_for(&a, "/tables/0/extra/reads/0/mode").is_none());
        // The fingerprint names the rules but not the fields present.
        assert!(
            a.fingerprint
                .contains("derivation=tables[].extra.reads[].mode by detector code")
        );
        assert!(!a.fingerprint.contains("field="));
        let mut v = serde_json::json!({"tables": [{"extra": {"reads": [{"mode": "luminescence"}, {"mode": "luminescence"}]}}]});
        assert_eq!(withhold(&mut v, &a), 1);
        assert_eq!(v["tables"][0]["extra"]["reads"][0]["mode"], "luminescence");
        assert!(v["tables"][0]["extra"]["reads"][1]["mode"].is_null());
    }

    #[test]
    fn a_field_no_oracle_compared_is_withheld_everywhere() {
        let p = profile_with(FIELD_ROWS);
        let a = assess_with(
            &p,
            "test",
            obs(&[
                (FeatureKind::FormatVersion, "1.0", &[Scope::Traces]),
                (FeatureKind::Field, "experiment.acquisition.started_at", &[]),
            ]),
            &ProvenanceMap::new(),
        );
        // Read along validated paths: the level stands, only the value is withheld.
        assert_eq!(a.level, AssuranceLevel::Validated);
        assert!(a.strict_refuses.is_empty());
        assert!(withheld_for(&a, "/experiment/acquisition/started_at").is_some());
        assert!(withheld_for(&a, "/experiment/acquisition").is_none());
        assert!(pattern_covers(
            "traces[].extra.acquired_at",
            "/traces/3/extra/acquired_at"
        ));
        assert!(!pattern_covers(
            "traces[].extra.acquired_at",
            "/traces/extra/acquired_at"
        ));
        assert_eq!(
            path_pointer("traces[0].extra.acquired_at"),
            "/traces/0/extra/acquired_at"
        );
        assert_eq!(
            generic_path("tables[2].extra.reads[10].mode"),
            "tables[].extra.reads[].mode"
        );
    }

    #[test]
    fn a_version_between_validated_versions_is_not_refused() {
        const VERSIONS: &[Validated] = &[
            Validated {
                kind: FeatureKind::FormatVersion,
                value: "2.4",
                files: 3,
                sources: 2,
                seen: 3,
            },
            Validated {
                kind: FeatureKind::FormatVersion,
                value: "2.9",
                files: 5,
                sources: 3,
                seen: 5,
            },
            Validated {
                kind: FeatureKind::FormatVersion,
                value: "3.1",
                files: 5,
                sources: 3,
                seen: 5,
            },
        ];
        let p = profile_with(VERSIONS);
        let a = assess_with(
            &p,
            "test",
            obs(&[(FeatureKind::FormatVersion, "2.6", &[Scope::Pixels])]),
            &ProvenanceMap::new(),
        );
        assert_eq!(a.level, AssuranceLevel::PartiallyValidated);
        assert!(a.reasons[0].contains("between the validated versions 2.4 and 2.9"));
        // Beyond the newest validated version, or in another major version: refused.
        for v in ["3.4", "1.9"] {
            let a = assess_with(
                &p,
                "test",
                obs(&[(FeatureKind::FormatVersion, v, &[Scope::Pixels])]),
                &ProvenanceMap::new(),
            );
            assert_eq!(a.strict_refuses, vec![Scope::Pixels], "{v}");
        }
        assert_eq!(
            version_key("ZEN 3.6.095"),
            Some(("zen".to_string(), vec![3, 6, 95]))
        );
    }

    #[test]
    fn assumed_values_are_withheld() {
        let p = profile_with(FIELD_ROWS);
        let mut o = obs(&[(FeatureKind::FormatVersion, "1.0", &[Scope::Tables])]);
        o.assumed("tables[].extra.acquired_at", "day/month order assumed");
        let a = assess_with(&p, "test", o, &ProvenanceMap::new());
        assert!(withheld_for(&a, "/tables/0/extra/acquired_at").is_some());
    }

    #[test]
    fn validated_when_every_feature_is_confirmed() {
        let p = profile_with(ROWS);
        let a = assess_with(
            &p,
            "test",
            obs(&[(FeatureKind::FormatVersion, "1.0", &[Scope::Pixels])]),
            &ProvenanceMap::new(),
        );
        assert_eq!(a.level, AssuranceLevel::Validated);
        assert!(a.strict_refuses.is_empty());
        assert_eq!(a.fingerprint, "test|format_version=1.0");
        assert_eq!(a.variant[0].corpus_files, 12);
    }

    #[test]
    fn unseen_structural_feature_is_unvalidated_and_refused() {
        let p = profile_with(ROWS);
        let a = assess_with(
            &p,
            "test",
            obs(&[
                (FeatureKind::FormatVersion, "1.0", &[Scope::Pixels]),
                (FeatureKind::Codec, "webp", &[Scope::Pixels]),
            ]),
            &ProvenanceMap::new(),
        );
        assert_eq!(a.level, AssuranceLevel::Unvalidated);
        assert_eq!(a.strict_refuses, vec![Scope::Pixels]);
        assert!(a.summary.starts_with("UNVALIDATED for pixels"));
        let e = a.strict_error("test", Scope::Pixels).unwrap();
        assert_eq!(e.exit_code(), 6);
        assert!(e.hint().unwrap().contains("--strict"));
        assert!(a.strict_error("test", Scope::Metadata).is_none());
    }

    #[test]
    fn seen_only_and_descriptive_unseen_are_partial() {
        let p = profile_with(ROWS);
        let a = assess_with(
            &p,
            "test",
            obs(&[
                (FeatureKind::Codec, "zstd", &[Scope::Pixels]),
                (FeatureKind::WriterVersion, "ZEN 9.9", &[]),
            ]),
            &ProvenanceMap::new(),
        );
        assert_eq!(a.level, AssuranceLevel::PartiallyValidated);
        assert!(a.strict_refuses.is_empty());
        assert_eq!(a.reasons.len(), 2);
    }

    #[test]
    fn undecoded_and_uncalibrated_refuse_their_scope() {
        let p = profile_with(ROWS);
        let mut o = obs(&[(FeatureKind::FormatVersion, "1.0", &[Scope::Spectra])]);
        o.calibration(
            "m/z",
            CalibrationStatus::NotApplied,
            &[Scope::Spectra],
            "stored coefficients not applied",
        );
        o.undecoded("thumbnail", &[], "not read");
        let a = assess_with(&p, "test", o, &ProvenanceMap::new());
        assert_eq!(a.level, AssuranceLevel::Unvalidated);
        assert_eq!(a.strict_refuses, vec![Scope::Spectra]);
        let mut o = obs(&[(FeatureKind::FormatVersion, "1.0", &[Scope::Spectra])]);
        o.calibration(
            "comp",
            CalibrationStatus::Available,
            &[Scope::Tables],
            "on request",
        );
        o.undecoded("thumbnail", &[], "not read");
        let a = assess_with(&p, "test", o, &ProvenanceMap::new());
        assert_eq!(a.level, AssuranceLevel::PartiallyValidated);
    }

    #[test]
    fn features_merge_scopes_and_skip_blanks() {
        let mut o = Observations::default();
        o.feature(FeatureKind::Codec, "jpeg", &[Scope::Pixels]);
        o.feature(FeatureKind::Codec, "jpeg", &[Scope::Metadata]);
        o.feature(FeatureKind::Codec, "  ", &[Scope::Pixels]);
        assert_eq!(o.features.len(), 1);
        assert_eq!(o.features[0].scope, vec![Scope::Metadata, Scope::Pixels]);
    }

    #[test]
    fn version_prefixes() {
        assert_eq!(version_prefix("3.6.095.00000", 2).as_deref(), Some("3.6"));
        assert_eq!(version_prefix("7,0,0,268", 2).as_deref(), Some("7.0"));
        assert_eq!(
            version_prefix("5.20.02 (Build 1453)", 2).as_deref(),
            Some("5.20")
        );
        assert_eq!(version_prefix("2.4 SP1", 2).as_deref(), Some("2.4"));
        assert_eq!(
            version_prefix("2.8-280502/2.8.1.2806", 2).as_deref(),
            Some("2.8")
        );
        assert_eq!(version_prefix("v1.2.3", 1).as_deref(), Some("1"));
        assert_eq!(version_prefix("LAS X", 2), None);
        assert_eq!(version_prefix("B.08.00", 2).as_deref(), Some("8.0"));
    }

    #[test]
    fn unprofiled_formats_are_unvalidated_everywhere() {
        let info = FileInfo {
            path: "x".into(),
            size_bytes: 1,
            format: crate::model::FormatDescriptor {
                id: "no-such-format-xyz".into(),
                name: "x".into(),
                vendor: "x".into(),
                extensions: vec![],
                family: "x".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            plane_count: 0,
            notes: vec![],
        };
        let a = assess(&info, Observations::default(), &ProvenanceMap::new());
        assert_eq!(a.level, AssuranceLevel::Unvalidated);
        assert_eq!(a.strict_refuses.len(), 5);
    }
}
