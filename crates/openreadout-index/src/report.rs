//! `openreadout report FILE`: a privacy-reviewed diagnostic bundle for a file that a reader
//! refused, failed on, or read without being able to validate it (`assurance.level` other than
//! `validated`).
//!
//! The bundle is what a maintainer needs to recognise a new variant (a vendor software release,
//! a codec, a layout) without the file itself:
//!
//! - the input's size, extension, signature bytes and SHA-256 (so a later public deposit of the
//!   same file can be matched to the report);
//! - which reader claimed it and every stage of the decode path (`detect`, `open`, `info`,
//!   `assurance`, `ls`, `check`, `vendor`, `first_read`) with its result, error code, message,
//!   hint and byte offset;
//! - the assurance block (fingerprint, unseen features, undecoded structures, assumptions);
//! - the structure map: `info --view structure` entries (kind, offset, size) and a key skeleton of
//!   the vendor metadata tree (paths and value types, no values);
//! - the normalized metadata with its numbers (sizes, dimensions, pixel types, units) but its free
//!   text replaced;
//! - optionally (`hex_bytes`) short hex excerpts of the file head and of structure headers, with
//!   printable text masked.
//!
//! What it never contains unless the user opts in (`include_text`): free text from the file
//! (sample, image and channel names, operators, comments, barcodes) and the path. It never contains
//! pixel, spectral, trace or table values at all: `first_read` decodes one plane, sweep, spectrum
//! or a few table rows only to report whether decoding works and the shape of the result. Personal
//! data flagged by the same rules as `info --view full --redact` ([`crate::pii`]) is replaced even
//! with `include_text`. Every string the bundle keeps from the file passes through one
//! [`Scrubber`], so a sample name removed from the metadata is removed from error messages,
//! structure entry names and vendor keys too. Nothing is sent anywhere: the caller writes the
//! bundle to a local file for the user to review and attach to an issue (`docs/maintaining.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use openreadout_core::assurance::Assurance;
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::region::Region;
use openreadout_core::{Error, FileInfo, Registry, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Version of the bundle layout (bumped when a key is renamed or removed).
pub const REPORT_VERSION: &str = "1";

/// Where new-variant reports go (printed, never contacted).
pub const ISSUE_URL: &str =
    "https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml";

/// Files larger than this are hashed over their first [`HASH_HEAD`] bytes only.
pub const HASH_WHOLE_LIMIT: u64 = 1 << 30;
/// Bytes hashed of a file above [`HASH_WHOLE_LIMIT`].
pub const HASH_HEAD: u64 = 64 << 20;
/// Largest hex excerpt, in bytes.
pub const MAX_HEX_BYTES: usize = 256;
/// Structure entries (`info --view structure`) listed one by one (the rest are only counted by
/// kind).
pub const MAX_ENTRIES: usize = 400;
/// Distinct vendor-tree key paths listed.
pub const MAX_VENDOR_PATHS: usize = 3000;
/// Members listed for a directory data set.
pub const MAX_MEMBERS: usize = 2000;
/// Elements kept of any array in the metadata and structure-entry details.
const MAX_ARRAY: usize = 16;
/// Structure headers excerpted with `hex_bytes`.
const MAX_HEX_SAMPLES: usize = 32;
/// Planes above this size are read as a 512 × 512 region by `first_read`.
const FIRST_READ_PLANE_BYTES: u64 = 64 << 20;

/// What goes into a report.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReportOptions {
    /// Keep free text from the file (sample, image and channel names, comments). Personal data
    /// flagged by [`crate::pii`] and the input path are still replaced.
    pub include_text: bool,
    /// Hex excerpts of this many bytes (at most [`MAX_HEX_BYTES`]) of the file head and of up to
    /// 32 structure headers; 0 (default) for none. Printable text runs are masked unless
    /// `include_text`.
    pub hex_bytes: usize,
    /// Run the full `check` (decodes and checksums data) instead of `check --headers-only`.
    pub full_check: bool,
    /// Record the input's SHA-256 (default true).
    pub hash: bool,
    /// Decode one plane, sweep, spectrum or a few table rows to see whether decoding works
    /// (default true). No value is kept.
    pub first_read: bool,
    /// Record how long each stage took (default true; tests turn it off for stable output).
    pub timings: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        ReportOptions {
            include_text: false,
            hex_bytes: 0,
            full_check: false,
            hash: true,
            first_read: true,
            timings: true,
        }
    }
}

/// The diagnostic bundle.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    /// Bundle layout version ([`REPORT_VERSION`]).
    pub report_version: String,
    /// The program that wrote it.
    pub generator: Generator,
    /// What the bundle contains and leaves out; read before sharing.
    pub privacy: Privacy,
    /// The input, without its name or path.
    pub input: InputSummary,
    /// The reader that claimed the input, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection: Option<DetectionSummary>,
    /// Readers registered for the input's extension (what the user probably expected).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
    /// The decode path: each stage in order with its result.
    pub stages: Vec<Stage>,
    /// The assurance block (`info` → `assurance`), text scrubbed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<Assurance>,
    /// The normalized metadata (`info`) with numbers kept and free text replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// The container's structure (`info --view structure`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<StructureMap>,
    /// Integrity findings (`check`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<Integrity>,
    /// Key skeleton of the vendor metadata tree (`info --view full` → `vendor`): paths and value
    /// types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_keys: Option<VendorKeys>,
    /// Hex excerpts (only with `hex_bytes`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<HexSample>,
}

/// Program name and version.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Generator {
    /// `openreadout`.
    pub name: String,
    /// Its version.
    pub version: String,
}

/// The privacy statement of a bundle.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Privacy {
    /// `structure_only` (default) or `with_text` (`--include-text`).
    pub mode: String,
    /// What the bundle contains, in plain words.
    pub included: Vec<String>,
    /// What it leaves out.
    pub excluded: Vec<String>,
    /// Distinct strings from the file replaced by `<redacted-N>` or `<text:LEN>` placeholders.
    pub redacted_strings: u32,
    /// Personal-data flags (kind and rule, never the value) found in the metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personal_data: Vec<String>,
}

/// The input without its name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct InputSummary {
    /// `file` or `directory`.
    pub kind: String,
    /// Bytes (a directory: the sum over its members).
    pub size_bytes: u64,
    /// Lower-case extension without the dot (`nd2`, `raw`, `d`), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// SHA-256 of the whole file (files up to 1 GiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// SHA-256 of the first 64 MiB (larger files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_first_64mib: Option<String>,
    /// The first 16 bytes in hex; printable runs that are not a plain token are masked (`__`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_hex: Option<String>,
    /// The same bytes as text where they are a plain token (`ZISRAWFILE`, `ABF2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_text: Option<String>,
    /// The file looks like text (a delimited export, XML, JSON).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text_like: bool,
    /// A directory's members: relative paths with names that are not plain tokens replaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    /// Members not listed (more than 2000).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub members_omitted: u64,
    /// Member counts per extension.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub member_extensions: BTreeMap<String, u64>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One member of a directory data set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Member {
    /// Path relative to the data set, `/`-separated.
    pub path: String,
    /// Bytes.
    pub size: u64,
}

/// The reader that claimed the input.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DetectionSummary {
    /// Format id.
    pub format: String,
    /// `definite`, `likely` or `extension_only`.
    pub confidence: String,
    /// Why the match is less than definite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The reader's confidence level (`high`, `medium`, `low`).
    pub reader_confidence: String,
}

/// One stage of the decode path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Stage {
    /// `detect`, `open`, `info`, `assurance`, `ls`, `check`, `vendor`, `first_read`.
    pub stage: String,
    /// `ok`, `error`, `skipped` or `panic` (a bug: the reader crashed in this stage).
    pub status: String,
    /// What the stage found, in a few words (`12 entries`, `plane 512x512 uint16`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The error, when the stage failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StageError>,
    /// Milliseconds the stage took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

/// An error as the CLI would report it, text scrubbed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StageError {
    /// Stable code (`corrupt_file`, `unsupported_feature`, ...).
    pub code: String,
    /// Process exit code the CLI would return.
    pub exit_code: i32,
    /// The message.
    pub message: String,
    /// The hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Byte offset of the problem, when the reader knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// The container's structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StructureMap {
    /// Entries `info --view structure` lists.
    pub entries_total: u64,
    /// Count and bytes per entry kind.
    pub by_kind: Vec<KindCount>,
    /// The first 400 entries in file order.
    pub entries: Vec<Value>,
}

/// Count and bytes of one entry kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct KindCount {
    /// Entry kind.
    pub kind: String,
    /// How many.
    pub count: u64,
    /// Their total size, where sizes are known.
    pub bytes: u64,
}

/// Integrity findings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Integrity {
    /// `headers` (`check --headers-only`) or `full`.
    pub mode: String,
    /// No finding of severity error.
    pub ok: bool,
    /// What was verified.
    pub checks_performed: Vec<String>,
    /// Findings: severity, code, message (scrubbed), offset.
    pub findings: Vec<Value>,
}

/// Key skeleton of the vendor tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct VendorKeys {
    /// Distinct key paths (array indices folded to `[]`).
    pub total_paths: u64,
    /// Paths not listed (beyond 3000).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted: u64,
    /// `path`, value `type` (`string`, `number`, `bool`, `null`, `object`, `array`), `count`.
    pub paths: Vec<VendorPath>,
}

/// One vendor key path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct VendorPath {
    /// Dotted key path, arrays as `[]`.
    pub path: String,
    /// JSON type of the values (several joined by `|`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Occurrences.
    pub count: u64,
}

/// A hex excerpt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct HexSample {
    /// `head` or the structure entry (`kind name`).
    pub at: String,
    /// Byte offset.
    pub offset: u64,
    /// Space-separated bytes; masked text bytes are `__`.
    pub hex: String,
}

/// What `openreadout report FILE` prints under `--json`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReportOutput {
    /// Where the bundle was written; `None` with `--dry-run` (or from MCP without `output`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Size of the written bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Where to file it (never contacted by the program).
    pub issue_url: String,
    /// The bundle, exactly as written.
    pub report: Report,
}

// ---------------------------------------------------------------------------------------------
// Scrubbing

/// Replaces text from the file everywhere in the bundle.
///
/// Strings are denied as they are removed from the metadata ([`Scrubber::deny_str`]); every
/// later string is passed through [`Scrubber::text`] (prose written by a reader: error messages,
/// hints, assurance reasons, check findings) or [`Scrubber::name`] (names taken from the file:
/// structure entry names, vendor keys, directory members), which replace each denied string with a
/// stable `<redacted-N>` placeholder. Distinctive strings (with a capital, a digit, a space or
/// punctuation: `Liver section 4`, `mouse7`) are replaced wherever they occur as a whole word.
/// Plain lower-case words (`liver`) are replaced in names, but in prose only where quoted
/// (`'liver'`), so that a sample called `spectra` does not blank out the word everywhere. The
/// input path and its specific components are always replaced.
#[derive(Debug, Clone, Default)]
pub struct Scrubber {
    include_text: bool,
    /// Exact substrings replaced anywhere (the input path, its directory, the home directory).
    paths: Vec<(String, String)>,
    /// Distinctive denied strings, longest first, with their placeholders.
    deny: Vec<(String, String)>,
    /// Plain lower-case denied words, longest first, with their placeholders.
    weak: Vec<(String, String)>,
    /// Every denied string.
    seen: BTreeSet<String>,
    /// Distinct strings from the file removed (denied or replaced by `<text:LEN>`).
    removed: BTreeSet<String>,
    /// Strings the metadata keeps (vocabulary and product names under [`KEEP_KEYS`]): never
    /// denied, and kept wherever else they occur in the metadata.
    kept: BTreeSet<String>,
}

/// Path components never denied (too generic to identify anyone, too common in messages).
const GENERIC_DIRS: &[&str] = &[
    "users",
    "home",
    "desktop",
    "documents",
    "downloads",
    "data",
    "tmp",
    "temp",
    "private",
    "var",
    "volumes",
    "mnt",
    "media",
    "files",
    "file",
    "raw",
    "export",
    "exports",
    "share",
    "shared",
    "projects",
    "project",
    "work",
    "scratch",
    "lab",
    "test",
    "tests",
    "corpus",
];

impl Scrubber {
    /// A scrubber for `input` (its path and every specific path component are denied).
    pub fn new(input: &Path, include_text: bool) -> Scrubber {
        let mut s = Scrubber {
            include_text,
            ..Scrubber::default()
        };
        let shown = input.display().to_string();
        let mut full: Vec<String> = vec![shown.clone()];
        if let Ok(c) = std::fs::canonicalize(input) {
            full.push(c.display().to_string());
        }
        for f in full {
            if let Some(parent) = Path::new(&f).parent().map(|p| p.display().to_string())
                && parent.len() > 1
            {
                s.paths.push((parent, "<dir>".into()));
            }
            s.paths.push((f, "<input>".into()));
        }
        if let Some(home) = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned())
            && home.len() > 1
        {
            s.paths.push((home, "<home>".into()));
        }
        // longest first, so the full path wins over its directory
        s.paths.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        s.paths.dedup();
        let mut comps: Vec<String> = Vec::new();
        for c in input.components() {
            let t = c.as_os_str().to_string_lossy().into_owned();
            comps.push(t.clone());
            // the stem too (`sample_7.nd2` → `sample_7`)
            if let Some(stem) = Path::new(&t).file_stem() {
                comps.push(stem.to_string_lossy().into_owned());
            }
        }
        for c in comps {
            if !GENERIC_DIRS.contains(&c.to_ascii_lowercase().as_str()) {
                s.deny_str(&c);
            }
        }
        // the path is not the file's content: not counted as removed
        s.removed.clear();
        s
    }

    /// Whether free text is kept (`include_text`).
    pub fn include_text(&self) -> bool {
        self.include_text
    }

    /// Deny `v` everywhere (at least 3 characters with a letter).
    pub fn deny_str(&mut self, v: &str) {
        let v = v.trim();
        if v.chars().count() < 3 || !v.chars().any(char::is_alphabetic) {
            return;
        }
        if self.kept.contains(v) || !self.seen.insert(v.to_string()) {
            return;
        }
        self.removed.insert(v.to_string());
        let placeholder = format!("<redacted-{}>", self.deny.len() + self.weak.len() + 1);
        let list = if v.chars().all(char::is_lowercase) {
            &mut self.weak
        } else {
            &mut self.deny
        };
        list.push((v.to_string(), placeholder));
        list.sort_by_key(|(d, _)| std::cmp::Reverse(d.chars().count()));
    }

    /// Keep `v` (a normalized variant feature value) wherever it occurs, unless it was already
    /// denied (personal data).
    pub fn keep(&mut self, v: &str) {
        let v = v.trim();
        if !v.is_empty() && !self.seen.contains(v) {
            self.kept.insert(v.to_string());
        }
    }

    /// Distinct strings from the file removed so far.
    pub fn redacted(&self) -> u32 {
        u32::try_from(self.removed.len()).unwrap_or(u32::MAX)
    }

    fn strong(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (p, rep) in &self.paths {
            if out.contains(p.as_str()) {
                out = out.replace(p.as_str(), rep);
            }
        }
        for (d, rep) in &self.deny {
            if out.contains(d.as_str()) {
                out = replace_words(&out, d, rep);
            }
        }
        out
    }

    /// Prose (a message a reader wrote): the path and distinctive denied strings replaced
    /// wherever they occur, plain denied words where quoted.
    pub fn text(&self, s: &str) -> String {
        let mut out = self.strong(s);
        for (w, rep) in &self.weak {
            if out.contains(w.as_str()) {
                for q in ['\'', '"', '`'] {
                    out = out.replace(&format!("{q}{w}{q}"), &format!("{q}{rep}{q}"));
                }
            }
        }
        out
    }

    /// A name taken from the file (a structure entry name, a vendor key, a member): every denied
    /// string replaced wherever it occurs as a whole word.
    pub fn name(&self, s: &str) -> String {
        let mut out = self.strong(s);
        for (w, rep) in &self.weak {
            if out.contains(w.as_str()) {
                out = replace_words(&out, w, rep);
            }
        }
        out
    }

    /// A key of the vendor tree or of a details object: kept when it is key-like, scrubbed.
    fn key(&self, k: &str) -> String {
        if key_like(k) || self.include_text {
            self.name(k)
        } else {
            format!("<key:{}>", k.chars().count())
        }
    }
}

/// Replace whole-word occurrences of `needle` (not preceded or followed by a letter or digit).
fn replace_words(hay: &str, needle: &str, rep: &str) -> String {
    let mut out = String::with_capacity(hay.len());
    let mut rest = hay;
    while let Some(i) = rest.find(needle) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + needle.len()..].chars().next();
        let bounded = before.is_none_or(|c| !c.is_alphanumeric())
            && after.is_none_or(|c| !c.is_alphanumeric());
        out.push_str(&rest[..i]);
        if bounded {
            out.push_str(rep);
        } else {
            out.push_str(needle);
        }
        rest = &rest[i + needle.len()..];
    }
    out.push_str(rest);
    out
}

/// Punctuation a structural name may contain (`ImageDataSeq|0!`, `AcqData/MSScan.bin`,
/// `[Image]`): anything but whitespace and quotes.
const TOKEN_PUNCT: &str = "_.:+/-#|!@$=,~^&*%[](){}<>";

/// A plain token: ASCII letters, digits and structural punctuation, no whitespace, at most
/// `max` characters.
fn token_like(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.chars().count() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || TOKEN_PUNCT.contains(c))
}

/// A key: a token that may also contain single spaces, `µ`, `°` and apostrophes (`Camera
/// Type`, `Temperature (°C)`), at most 64 characters.
fn key_like(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= 64
        && !s.contains("  ")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || TOKEN_PUNCT.contains(c) || " µ°'".contains(c))
}

/// A number with an optional short unit (`50 ms`, `-70.0°C`, `17 MHz`, `3.3 µs`) or a
/// binning (`1x1`, `2 x 2`): a measurement, not text a person wrote.
fn numeric_like(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 16 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    if let Some((a, b)) = lower.split_once('x')
        && a.trim().parse::<u32>().is_ok()
        && b.trim().parse::<u32>().is_ok()
    {
        return true;
    }
    let num_end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || ((*c == '-' || *c == '+') && *i == 0)))
        .map_or(t.len(), |(i, _)| i);
    let (num, unit) = t.split_at(num_end);
    num.parse::<f64>().is_ok()
        && unit.trim().chars().count() <= 6
        && unit
            .trim()
            .chars()
            .all(|c| c.is_alphabetic() || "°%/".contains(c))
}

/// An ISO-8601 date or date-time: its year.
fn iso_year(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    (b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit))
    .then(|| &s[..4])
}

/// Keys whose string values in the normalized metadata are vocabulary or vendor product names,
/// never free text from the user: kept as they are.
const KEEP_KEYS: &[&str] = &[
    "pixel_type",
    "dimension_order",
    "format_version",
    "unit",
    "units",
    "family",
    "codec",
    "compression",
    "dtype",
    "data_type",
    "byte_order",
    "endianness",
    "layout",
    "mode",
    "acquisition_mode",
    "polarity",
    "activation",
    "color",
    "manufacturer",
    "vendor",
    "software",
    "software_version",
    "model",
    "immersion",
    "correction",
    "encoding",
    "state",
    "status",
    "view",
    "dialect",
    "delimiter",
    "container",
    "technique_term",
    "axis",
    "scale",
    "read_mode",
    "detection",
    "ms_levels",
    "scan_filter",
    "kind",
    "ucum",
];

/// Key fragments whose values identify a person or a machine even when they look like numbers
/// or tokens (host names, paths, accounts): removed even with `include_text`.
const DROP_ALWAYS: &[&str] = &[
    "host",
    "computer",
    "machine_name",
    "path",
    "user",
    "operator",
    "owner",
    "email",
    "account",
];

/// Key fragments whose values identify an instrument, a lab or a sample (serial numbers,
/// barcodes, licences, file names): removed unless `include_text`.
const DROP_DEFAULT: &[&str] = &["serial", "barcode", "license", "licence", "file"];

/// Whether values under `key` are removed (numbers too).
fn drop_value(key: &str, include_text: bool) -> bool {
    let k = key.to_ascii_lowercase();
    DROP_ALWAYS.iter().any(|p| k.contains(p))
        || (!include_text && DROP_DEFAULT.iter().any(|p| k.contains(p)))
}

/// Key suffixes that mark vocabulary values.
const KEEP_SUFFIXES: &[&str] = &[
    "_unit",
    "_units",
    "_type",
    "_mode",
    "_kind",
    "_version",
    "_format",
    "_order",
    "_codec",
    "_source",
    "_dialect",
    "_encoding",
    "_layout",
];

/// Parents under which `id` and `label` are ontology terms (`OBI:0400169`, `microscope`).
const TERM_PARENTS: &[&str] = &[
    "kind",
    "technique",
    "assay",
    "instrument_kind",
    "format",
    "terms",
];

fn keep_string(key: &str, parent: &str) -> bool {
    let k = key.to_ascii_lowercase();
    !drop_value(key, false) && KEEP_KEYS.contains(&k.as_str())
        || KEEP_SUFFIXES.iter().any(|s| k.ends_with(s))
        || (matches!(k.as_str(), "id" | "label" | "name") && TERM_PARENTS.contains(&parent))
}

impl Scrubber {
    /// Remember the strings the metadata keeps by key, so the same value under another key
    /// (an objective model repeated as an experiment parameter) is kept too, not denied.
    fn collect_kept(&mut self, v: &Value, key: &str, parent: &str) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    self.collect_kept(x, k, key);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| self.collect_kept(x, key, parent)),
            Value::String(s) if keep_string(key, parent) && !s.trim().is_empty() => {
                // numbers and tokens under a dropped key are never kept
                self.kept.insert(s.trim().to_string());
            }
            _ => {}
        }
    }

    /// The normalized metadata: numbers and vocabulary kept, other strings replaced by
    /// `<text:LEN>` (and denied everywhere), dates reduced to their year, arrays capped.
    fn shape_info(&mut self, v: &Value, key: &str, parent: &str) -> Value {
        match v {
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    out.insert(self.key(k), self.shape_info(x, k, key));
                }
                Value::Object(out)
            }
            Value::Array(a) => {
                let mut out: Vec<Value> = a
                    .iter()
                    .take(MAX_ARRAY)
                    .map(|x| self.shape_info(x, key, parent))
                    .collect();
                if a.len() > MAX_ARRAY {
                    out.push(Value::from(format!("<{} more>", a.len() - MAX_ARRAY)));
                }
                Value::Array(out)
            }
            Value::Number(_) if drop_value(key, self.include_text) => Value::from("<number>"),
            Value::String(s) => {
                let dropped = drop_value(key, self.include_text);
                if !dropped
                    && (self.include_text
                        || keep_string(key, parent)
                        || self.kept.contains(s.trim())
                        || numeric_like(s))
                {
                    Value::from(self.text(s))
                } else if let Some(y) = iso_year(s) {
                    Value::from(format!("<date:{y}>"))
                } else if s.is_empty() {
                    Value::from("")
                } else {
                    self.removed.insert(s.clone());
                    self.deny_str(s);
                    Value::from(format!("<text:{}>", s.chars().count()))
                }
            }
            other => other.clone(),
        }
    }

    /// Structure-entry details and other reader-written structure: numbers kept, plain tokens kept
    /// (after scrubbing), other strings replaced.
    fn shape_details(&mut self, v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    out.insert(self.key(k), self.shape_details(x));
                }
                Value::Object(out)
            }
            Value::Array(a) => {
                let mut out: Vec<Value> = a
                    .iter()
                    .take(MAX_ARRAY)
                    .map(|x| self.shape_details(x))
                    .collect();
                if a.len() > MAX_ARRAY {
                    out.push(Value::from(format!("<{} more>", a.len() - MAX_ARRAY)));
                }
                Value::Array(out)
            }
            Value::String(s) => {
                if self.include_text || token_like(s, 40) || numeric_like(s) {
                    Value::from(self.name(s))
                } else if let Some(y) = iso_year(s) {
                    Value::from(format!("<date:{y}>"))
                } else {
                    self.removed.insert(s.clone());
                    Value::from(format!("<text:{}>", s.chars().count()))
                }
            }
            other => other.clone(),
        }
    }

    /// Scrub every string (not key) of an already-shaped value.
    fn scrub_value(&self, v: &mut Value) {
        match v {
            Value::Object(m) => m.values_mut().for_each(|x| self.scrub_value(x)),
            Value::Array(a) => a.iter_mut().for_each(|x| self.scrub_value(x)),
            Value::String(s) => *s = self.text(s),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Panic capture

/// A report being built, for the panic hook of the CLI ([`panic_hook`]).
struct Active {
    snapshot: Report,
    scrubber: Scrubber,
    output: Option<PathBuf>,
    overwrite: bool,
    stage: String,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn set_active(f: impl FnOnce(&mut Option<Active>)) {
    if let Ok(mut a) = ACTIVE.lock() {
        f(&mut a);
    }
}

/// Called by the CLI's panic hook: when a report is being built, finish it with a `panic`
/// stage (the message scrubbed like everything else), write it where the report was going to
/// be written, and return that path. `None` when no report is in progress or it has no output.
pub fn panic_hook(message: &str) -> Option<PathBuf> {
    // try_lock: the hook must never wait (the panic may have happened while it was held).
    let mut guard = ACTIVE.try_lock().ok()?;
    let active = guard.take()?;
    let mut r = active.snapshot;
    r.stages.push(Stage {
        stage: active.stage.clone(),
        status: "panic".into(),
        detail: Some(
            "the reader crashed (a bug); this report was written by the panic handler".into(),
        ),
        error: Some(StageError {
            code: "internal_panic".into(),
            exit_code: 1,
            message: active.scrubber.text(message),
            hint: Some(
                "Please file this report: a reader must refuse a file it cannot read, never crash."
                    .into(),
            ),
            offset: None,
        }),
        elapsed_ms: None,
    });
    let path = active.output?;
    let bytes = serde_json::to_vec_pretty(&r).ok()?;
    write_verified(&path, &bytes, active.overwrite).ok()?;
    Some(path)
}

// ---------------------------------------------------------------------------------------------
// Building

struct Builder<'a> {
    opts: &'a ReportOptions,
    report: Report,
    scrubber: Scrubber,
    /// Raw stage errors, scrubbed at the end (the scrubber grows as the metadata is read).
    raw_errors: Vec<Option<RawError>>,
}

struct RawError {
    code: String,
    exit_code: i32,
    message: String,
    hint: Option<String>,
    offset: Option<u64>,
}

impl From<&Error> for RawError {
    fn from(e: &Error) -> Self {
        RawError {
            code: e.code().into(),
            exit_code: e.exit_code(),
            message: e.to_string(),
            hint: e.hint(),
            offset: match e {
                Error::Corrupt { offset, .. } => *offset,
                _ => None,
            },
        }
    }
}

impl Builder<'_> {
    fn begin(&self, stage: &str) -> Instant {
        let snapshot = self.scrubbed_snapshot();
        let scrubber = self.scrubber.clone();
        set_active(|a| {
            if let Some(a) = a.as_mut() {
                a.snapshot = snapshot;
                a.scrubber = scrubber;
                a.stage = stage.to_string();
            }
        });
        Instant::now()
    }

    fn end(
        &mut self,
        stage: &str,
        t: Instant,
        result: std::result::Result<Option<String>, &Error>,
    ) {
        let elapsed = self
            .opts
            .timings
            .then(|| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX));
        let (status, detail, raw) = match result {
            Ok(d) => ("ok", d, None),
            Err(e) => ("error", None, Some(RawError::from(e))),
        };
        self.report.stages.push(Stage {
            stage: stage.into(),
            status: status.into(),
            detail,
            error: None,
            elapsed_ms: elapsed,
        });
        self.raw_errors.push(raw);
    }

    fn skip(&mut self, stage: &str, why: &str) {
        self.report.stages.push(Stage {
            stage: stage.into(),
            status: "skipped".into(),
            detail: Some(why.into()),
            error: None,
            elapsed_ms: None,
        });
        self.raw_errors.push(None);
    }

    /// The report so far with every stage error scrubbed.
    fn scrubbed_snapshot(&self) -> Report {
        let mut r = self.report.clone();
        for (stage, raw) in r.stages.iter_mut().zip(&self.raw_errors) {
            if let Some(e) = raw {
                stage.error = Some(StageError {
                    code: e.code.clone(),
                    exit_code: e.exit_code,
                    message: self.scrubber.text(&e.message),
                    hint: e.hint.as_deref().map(|h| self.scrubber.text(h)),
                    offset: e.offset,
                });
            }
            if let Some(d) = &stage.detail {
                stage.detail = Some(self.scrubber.text(d));
            }
        }
        r
    }
}

/// Build the report for `path`. `output`, when given, is where the CLI will write it: the panic
/// hook writes the partial report there if a reader crashes on the way.
///
/// Errors only when the input cannot be read at all (it does not exist: exit 5); every reader
/// failure is recorded as a stage of the report instead.
pub fn build(
    reg: &Registry,
    path: &Path,
    opts: &ReportOptions,
    output: Option<(&Path, bool)>,
) -> Result<Report> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
    let mut b = Builder {
        opts,
        report: Report {
            report_version: REPORT_VERSION.into(),
            generator: Generator {
                name: "openreadout".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            ..Report::default()
        },
        scrubber: Scrubber::new(path, opts.include_text),
        raw_errors: Vec::new(),
    };
    b.report.input = input_summary(path, &meta, opts)?;
    b.report.candidates = candidates(reg, path);
    set_active(|a| {
        *a = Some(Active {
            snapshot: b.report.clone(),
            scrubber: b.scrubber.clone(),
            output: output.map(|(p, _)| p.to_path_buf()),
            overwrite: output.is_some_and(|(_, o)| o),
            stage: "detect".into(),
        });
    });
    run_stages(reg, path, &mut b);
    set_active(|a| *a = None);
    Ok(b.finish())
}

#[allow(clippy::many_single_char_names)]
fn run_stages(reg: &Registry, path: &Path, b: &mut Builder<'_>) {
    // detect
    let t = b.begin("detect");
    let reader = match reg.detect(path) {
        Ok((r, det)) => {
            let d = r.descriptor();
            b.report.detection = Some(DetectionSummary {
                format: det.format_id.to_string(),
                confidence: token(&det.confidence),
                note: det.note.clone(),
                reader_confidence: token(&d.confidence),
            });
            b.end(
                "detect",
                t,
                Ok(Some(format!("{} ({})", det.format_id, d.name))),
            );
            r
        }
        Err(e) => {
            b.end("detect", t, Err(&e));
            for s in STAGES_AFTER_DETECT {
                b.skip(s, "no reader claimed the input");
            }
            return;
        }
    };
    // open
    let t = b.begin("open");
    let mut ds: Box<dyn Dataset> = match reader.open(path) {
        Ok(ds) => {
            b.end("open", t, Ok(None));
            ds
        }
        Err(e) => {
            b.end("open", t, Err(&e));
            for s in &STAGES_AFTER_DETECT[1..] {
                b.skip(s, "the reader could not open the input");
            }
            return;
        }
    };
    // info (and the vendor tree, read now so personal data found there is denied everywhere)
    let t = b.begin("info");
    let info = match ds.info() {
        Ok(i) => {
            let d = info_detail(&i);
            b.end("info", t, Ok(Some(d)));
            Some(i)
        }
        Err(e) => {
            b.end("info", t, Err(&e));
            None
        }
    };
    let t = b.begin("vendor");
    let vendor = match ds.vendor_metadata() {
        Ok(v) => {
            b.end("vendor", t, Ok(None));
            Some(v)
        }
        Err(e) => {
            b.end("vendor", t, Err(&e));
            None
        }
    };
    // assurance: its variant features are normalized by the reader's profile (format versions,
    // writers, codecs): vocabulary the rest of the bundle keeps
    let t = b.begin("assurance");
    let assessed = if let Some(i) = &info {
        let a = openreadout_core::assurance::assess_dataset(ds.as_ref(), i);
        let d = format!("{} ({} features)", a.level.as_str(), a.variant.len());
        b.end("assurance", t, Ok(Some(d)));
        Some(a)
    } else {
        b.skip("assurance", "info failed");
        None
    };
    // Deny personal data wherever it was found, then shape the metadata (which denies every
    // free-text string it removes).
    let info_json = info.as_ref().map(|i| {
        let out = openreadout_core::InfoOutput::new(ds.as_ref(), i.clone());
        serde_json::to_value(&out).unwrap_or(Value::Null)
    });
    let mut flags = BTreeSet::new();
    for doc in [info_json.as_ref(), vendor.as_ref()].into_iter().flatten() {
        for f in crate::pii::scan(doc) {
            flags.insert(format!("{} ({})", f.kind, f.rule));
            b.scrubber.deny_str(&f.value);
        }
    }
    b.report.privacy.personal_data = flags.into_iter().collect();
    for f in assessed.iter().flat_map(|a| &a.variant) {
        b.scrubber.keep(&f.value);
    }
    if let Some(mut j) = info_json {
        if let Some(m) = j.as_object_mut() {
            m.remove("path");
            // the descriptor is the same for every file of the format: keep id and name only
            if let Some(f) = m.get("format").cloned() {
                m.insert(
                    "format".into(),
                    serde_json::json!({"id": f["id"], "name": f["name"]}),
                );
            }
            m.remove("assurance");
        }
        b.scrubber.collect_kept(&j, "", "");
        let notes = j.get("notes").cloned();
        let mut shaped = b.scrubber.shape_info(&j, "", "");
        // the reader's notes are prose worth keeping: scrubbed, not replaced
        if let (Some(Value::Array(n)), Some(m)) = (notes, shaped.as_object_mut()) {
            let scrubbed: Vec<Value> = n
                .iter()
                .filter_map(Value::as_str)
                .map(|t| Value::from(b.scrubber.text(t)))
                .collect();
            m.insert("notes".into(), Value::Array(scrubbed));
        }
        b.report.metadata = Some(shaped);
    }
    if let Some(mut a) = assessed {
        scrub_assurance(&b.scrubber, &mut a);
        b.report.assurance = Some(a);
    }
    // ls
    let t = b.begin("ls");
    match ds.entries() {
        Ok(entries) => {
            let n = entries.len();
            b.report.structure = Some(structure_map(&mut b.scrubber, &entries));
            b.end("ls", t, Ok(Some(format!("{n} entries"))));
        }
        Err(e) => b.end("ls", t, Err(&e)),
    }
    // check
    let t = b.begin("check");
    let checked = if b.opts.full_check {
        ds.check()
    } else {
        ds.check_headers()
    };
    match checked {
        Ok(r) => {
            let findings: Vec<Value> = r
                .findings
                .iter()
                .map(|f| {
                    let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
                    b.scrubber.scrub_value(&mut v);
                    v
                })
                .collect();
            let d = format!(
                "{} ({} finding{})",
                if r.ok { "ok" } else { "not ok" },
                findings.len(),
                if findings.len() == 1 { "" } else { "s" }
            );
            b.report.integrity = Some(Integrity {
                mode: if b.opts.full_check { "full" } else { "headers" }.into(),
                ok: r.ok,
                checks_performed: r
                    .checks_performed
                    .iter()
                    .map(|c| b.scrubber.text(c))
                    .collect(),
                findings,
            });
            b.end("check", t, Ok(Some(d)));
        }
        Err(e) => b.end("check", t, Err(&e)),
    }
    // first read
    if !b.opts.first_read {
        b.skip("first_read", "turned off");
    } else if let Some(i) = &info {
        let t = b.begin("first_read");
        match first_read(ds.as_mut(), i) {
            Ok(Some(d)) => b.end("first_read", t, Ok(Some(d))),
            Ok(None) => b.skip("first_read", "no image, trace, spectrum or table to read"),
            Err(e) => b.end("first_read", t, Err(&e)),
        }
    } else {
        b.skip("first_read", "info failed");
    }
    // vendor keys
    if let Some(v) = &vendor {
        b.report.vendor_keys = Some(vendor_keys(&b.scrubber, v));
    }
    // hex samples
    if b.opts.hex_bytes > 0 && b.report.input.kind == "file" {
        let entries = ds.entries().unwrap_or_default();
        b.report.samples = hex_samples(path, &entries, b.opts, &b.scrubber);
    }
}

const STAGES_AFTER_DETECT: [&str; 7] = [
    "open",
    "info",
    "vendor",
    "assurance",
    "ls",
    "check",
    "first_read",
];

impl Builder<'_> {
    fn finish(self) -> Report {
        let mut r = self.scrubbed_snapshot();
        if let Some(d) = r.detection.as_mut() {
            d.note = d.note.as_deref().map(|n| self.scrubber.text(n));
        }
        // signature bytes are scrubbed last, against everything denied
        if let Some(sig) = &r.input.signature_text {
            let s = self.scrubber.name(sig);
            r.input.signature_text = (s == *sig).then_some(s);
        }
        for m in &mut r.input.members {
            m.path = m
                .path
                .split('/')
                .map(|c| self.scrubber.name(c))
                .collect::<Vec<_>>()
                .join("/");
        }
        let include = self.opts.include_text;
        r.privacy.mode = if include {
            "with_text"
        } else {
            "structure_only"
        }
        .into();
        r.privacy.redacted_strings = self.scrubber.redacted();
        r.privacy.included = vec![
            "the input's size, extension, first 16 bytes (text masked) and SHA-256".into(),
            "which reader claimed the input and the result of every decode stage (error codes, messages, hints, byte offsets)".into(),
            "the assurance block: the variant fingerprint and what is unvalidated".into(),
            "the structure map: `info --view structure` entries (kind, offset, size) and the key names of the vendor metadata tree".into(),
            "the normalized metadata's numbers: dimensions, pixel types, units, physical sizes, counts".into(),
        ];
        if include {
            r.privacy
                .included
                .push("free text from the file: sample, image and channel names, comments (--include-text)".into());
        }
        if self.opts.hex_bytes > 0 {
            r.privacy.included.push(format!(
                "hex excerpts of up to {} bytes at the file head and at structure headers{}",
                self.opts.hex_bytes.min(MAX_HEX_BYTES),
                if include {
                    ""
                } else {
                    ", printable text masked"
                }
            ));
        }
        r.privacy.excluded = vec![
            "pixel, spectral, trace and table values (first_read reports only whether decoding works and the shape)".into(),
            "the file's name and path".into(),
            "values of the vendor metadata tree".into(),
            "personal data (operators, e-mail addresses, phone numbers, patient identifiers, dates of birth)".into(),
        ];
        if !include {
            r.privacy.excluded.push(
                "free text from the file (sample, image and channel names, comments, barcodes): replaced by <text:LEN> or <redacted-N>; dates reduced to the year".into(),
            );
        }
        r
    }
}

fn token<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn info_detail(i: &FileInfo) -> String {
    let mut parts = Vec::new();
    if !i.images.is_empty() {
        parts.push(format!(
            "{} images, {} planes",
            i.images.len(),
            i.plane_count
        ));
    }
    if !i.traces.is_empty() {
        parts.push(format!("{} traces", i.traces.len()));
    }
    if !i.spectra.is_empty() {
        let scans: u64 = i.spectra.iter().map(|s| s.scan_count).sum();
        parts.push(format!("{} runs, {scans} spectra", i.spectra.len()));
    }
    if !i.tables.is_empty() {
        parts.push(format!("{} tables", i.tables.len()));
    }
    if parts.is_empty() {
        "no images, traces, spectra or tables".into()
    } else {
        parts.join(", ")
    }
}

fn scrub_assurance(s: &Scrubber, a: &mut Assurance) {
    a.summary = s.text(&a.summary);
    a.fingerprint = s.text(&a.fingerprint);
    for f in &mut a.variant {
        f.value = s.text(&f.value);
    }
    for r in &mut a.reasons {
        *r = s.text(r);
    }
    for u in &mut a.undecoded {
        u.structure = s.text(&u.structure);
        u.detail = s.text(&u.detail);
    }
    for x in &mut a.assumed {
        x.field = s.text(&x.field);
        x.detail = s.text(&x.detail);
    }
    for c in &mut a.calibrations {
        c.name = s.text(&c.name);
        c.detail = s.text(&c.detail);
    }
    for e in &mut a.inferred_fields.examples {
        *e = s.text(e);
    }
}

fn candidates(reg: &Registry, path: &Path) -> Vec<String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    reg.descriptors()
        .into_iter()
        .filter(|d| {
            d.extensions
                .iter()
                .any(|e| name.ends_with(&format!(".{e}")))
        })
        .map(|d| d.id)
        .collect()
}

fn input_summary(
    path: &Path,
    meta: &std::fs::Metadata,
    opts: &ReportOptions,
) -> Result<InputSummary> {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| token_like(e, 12));
    if meta.is_dir() {
        let mut s = InputSummary {
            kind: "directory".into(),
            extension,
            ..InputSummary::default()
        };
        let mut stack = vec![(path.to_path_buf(), 0usize)];
        let mut members: Vec<(String, u64)> = Vec::new();
        let mut omitted = 0u64;
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<_> = rd.filter_map(std::result::Result::ok).collect();
            children.sort_by_key(std::fs::DirEntry::file_name);
            for c in children {
                let p = c.path();
                let Ok(m) = c.metadata() else { continue };
                if m.is_dir() {
                    if depth < 8 {
                        stack.push((p, depth + 1));
                    }
                    continue;
                }
                s.size_bytes = s.size_bytes.saturating_add(m.len());
                let ext = p
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .filter(|e| token_like(e, 12))
                    .unwrap_or_else(|| "(none)".into());
                *s.member_extensions.entry(ext).or_default() += 1;
                if members.len() < MAX_MEMBERS {
                    let rel = p
                        .strip_prefix(path)
                        .unwrap_or(&p)
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    members.push((rel, m.len()));
                } else {
                    omitted += 1;
                }
            }
        }
        members.sort();
        // Member names are scrubbed later against everything denied; names that are not plain
        // tokens are replaced now.
        s.members = members
            .into_iter()
            .map(|(p, size)| Member {
                path: p
                    .split('/')
                    .map(|c| {
                        if opts.include_text {
                            c.to_string()
                        } else {
                            member_name(c)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("/"),
                size,
            })
            .collect();
        s.members_omitted = omitted;
        return Ok(s);
    }
    let mut f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut head = vec![0u8; 4096];
    let mut filled = 0;
    while filled < head.len() {
        let n = f
            .read(&mut head[filled..])
            .map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    head.truncate(filled);
    let sig = &head[..head.len().min(16)];
    let (hex, text) = signature(sig, opts.include_text);
    let text_like = !head.is_empty()
        && head
            .iter()
            .filter(|b| b.is_ascii_graphic() || b.is_ascii_whitespace() || **b >= 0x80)
            .count()
            * 100
            / head.len()
            >= 97
        && !head.contains(&0);
    let mut s = InputSummary {
        kind: "file".into(),
        size_bytes: meta.len(),
        extension,
        signature_hex: Some(hex),
        signature_text: text,
        text_like,
        ..InputSummary::default()
    };
    if opts.hash {
        f.seek(SeekFrom::Start(0)).map_err(|e| Error::io(path, e))?;
        let limit = if meta.len() > HASH_WHOLE_LIMIT {
            HASH_HEAD
        } else {
            u64::MAX
        };
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut left = limit;
        while left > 0 {
            let want = usize::try_from(left.min(buf.len() as u64)).unwrap_or(buf.len());
            let n = f.read(&mut buf[..want]).map_err(|e| Error::io(path, e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            left -= n as u64;
        }
        let digest = hex_string(&h.finalize());
        if meta.len() > HASH_WHOLE_LIMIT {
            s.sha256_first_64mib = Some(digest);
        } else {
            s.sha256 = Some(digest);
        }
    }
    Ok(s)
}

fn hex_string(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// Mask runs of 4+ printable bytes that are not a plain token (they may be text a person wrote).
fn masked(bytes: &[u8], include_text: bool) -> Vec<bool> {
    let mut mask = vec![false; bytes.len()];
    if include_text {
        return mask;
    }
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_graphic() || bytes[i] == b' ' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_graphic() || bytes[i] == b' ') {
                i += 1;
            }
            let run = &bytes[start..i];
            let text = String::from_utf8_lossy(run);
            if run.len() >= 4 && !token_like(&text, 64) {
                mask[start..i].iter_mut().for_each(|m| *m = true);
            }
        } else {
            i += 1;
        }
    }
    // UTF-16LE text (`S\0a\0m\0p\0l\0e\0`), common in vendor binaries: 4+ characters of
    // (printable, 0) pairs are masked whole
    let mut i = 0;
    while i + 1 < bytes.len() {
        let start = i;
        while i + 1 < bytes.len()
            && (bytes[i].is_ascii_graphic() || bytes[i] == b' ')
            && bytes[i + 1] == 0
        {
            i += 2;
        }
        if i - start >= 8 {
            mask[start..i].iter_mut().for_each(|m| *m = true);
        }
        if i == start {
            i += 1;
        }
    }
    mask
}

/// A directory member's name as shared: numeric names (`1`, `pdata/1`) and short letter-only
/// names (`acqus`, `analysis.tdf`, `MSScan.bin`) are the vendor's structure and kept; any other
/// name may be one a person chose (`Mouse7_liver.tif`) and is reduced to its shape: letters to
/// `a`/`A`, digits to `9`, the extension kept (`Aaaaa9_aaaaa.tif`).
fn member_name(c: &str) -> String {
    let (stem, ext) = match c.rsplit_once('.') {
        Some((st, e)) if !st.is_empty() && token_like(e, 8) => (st, Some(e)),
        _ => (c, None),
    };
    let numeric = !stem.is_empty() && stem.chars().all(|ch| ch.is_ascii_digit());
    let plain = stem.chars().count() <= 16
        && stem.chars().any(|ch| ch.is_ascii_alphabetic())
        && stem
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || "_-.".contains(ch));
    if numeric || plain {
        return c.to_string();
    }
    let shape: String = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_uppercase() {
                'A'
            } else if ch.is_alphabetic() {
                'a'
            } else if ch.is_ascii_digit() {
                '9'
            } else {
                ch
            }
        })
        .collect();
    ext.map_or(shape.clone(), |e| format!("{shape}.{e}"))
}

fn signature(sig: &[u8], include_text: bool) -> (String, Option<String>) {
    let mask = masked(sig, include_text);
    let hex = sig
        .iter()
        .zip(&mask)
        .map(|(b, m)| {
            if *m {
                "__".to_string()
            } else {
                format!("{b:02x}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    // the longest leading printable token, as text (`ZISRAWFILE`, `ABF2`, `<?xml`)
    let lead: String = sig
        .iter()
        .zip(&mask)
        .take_while(|(b, m)| !**m && b.is_ascii_graphic())
        .map(|(b, _)| char::from(*b))
        .collect();
    (hex, (lead.len() >= 2).then_some(lead))
}

fn structure_map(sc: &mut Scrubber, entries: &[openreadout_core::LsEntry]) -> StructureMap {
    let mut by: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    // Entry kinds are the reader's own vocabulary (`chunk`, `frame`, `attachment`).
    let kind = |k: &str| {
        if token_like(k, 40) {
            k.to_string()
        } else {
            format!("<kind:{}>", k.chars().count())
        }
    };
    for e in entries {
        let c = by.entry(kind(&e.kind)).or_default();
        c.0 += 1;
        c.1 = c.1.saturating_add(e.size.unwrap_or(0));
    }
    let mut out = Vec::new();
    for e in entries.iter().take(MAX_ENTRIES) {
        let mut m = Map::new();
        m.insert("kind".into(), Value::from(kind(&e.kind)));
        let name = if sc.include_text() || token_like(&e.name, 96) {
            sc.name(&e.name)
        } else {
            format!("<name:{}>", e.name.chars().count())
        };
        m.insert("name".into(), Value::from(name));
        if let Some(o) = e.offset {
            m.insert("offset".into(), Value::from(o));
        }
        if let Some(s) = e.size {
            m.insert("size".into(), Value::from(s));
        }
        if let Some(i) = e.image {
            m.insert("image".into(), Value::from(i));
        }
        if !e.details.is_null() {
            let d = sc.shape_details(&e.details);
            m.insert("details".into(), d);
        }
        out.push(Value::Object(m));
    }
    StructureMap {
        entries_total: entries.len() as u64,
        by_kind: by
            .into_iter()
            .map(|(kind, (count, bytes))| KindCount { kind, count, bytes })
            .collect(),
        entries: out,
    }
}

#[allow(clippy::many_single_char_names)]
fn vendor_keys(sc: &Scrubber, v: &Value) -> VendorKeys {
    fn walk(
        sc: &Scrubber,
        v: &Value,
        path: &str,
        out: &mut BTreeMap<String, (BTreeSet<&'static str>, u64)>,
    ) {
        let t = match v {
            Value::Object(_) => "object",
            Value::Array(_) => "array",
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::Null => "null",
        };
        if !path.is_empty() {
            let e = out.entry(path.to_string()).or_default();
            e.0.insert(t);
            e.1 += 1;
        }
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    let k = sc.key(k);
                    let p = if path.is_empty() {
                        k
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(sc, x, &p, out);
                }
            }
            Value::Array(a) => {
                let p = format!("{path}[]");
                for x in a {
                    walk(sc, x, &p, out);
                }
            }
            _ => {}
        }
    }
    let mut paths = BTreeMap::new();
    walk(sc, v, "", &mut paths);
    let total = paths.len() as u64;
    let list: Vec<VendorPath> = paths
        .into_iter()
        .take(MAX_VENDOR_PATHS)
        .map(|(path, (types, count))| VendorPath {
            path,
            kind: types.into_iter().collect::<Vec<_>>().join("|"),
            count,
        })
        .collect();
    VendorKeys {
        total_paths: total,
        omitted: total.saturating_sub(list.len() as u64),
        paths: list,
    }
}

fn hex_samples(
    path: &Path,
    entries: &[openreadout_core::LsEntry],
    opts: &ReportOptions,
    sc: &Scrubber,
) -> Vec<HexSample> {
    let n = opts.hex_bytes.min(MAX_HEX_BYTES);
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut at: Vec<(String, u64)> = vec![("head".into(), 0)];
    let mut offsets = BTreeSet::from([0u64]);
    for e in entries {
        if at.len() > MAX_HEX_SAMPLES {
            break;
        }
        if let Some(o) = e.offset
            && offsets.insert(o)
        {
            let name = if token_like(&e.name, 96) {
                sc.name(&e.name)
            } else {
                format!("<name:{}>", e.name.chars().count())
            };
            let kind = if token_like(&e.kind, 40) {
                e.kind.as_str()
            } else {
                "entry"
            };
            at.push((format!("{kind} {name}"), o));
        }
    }
    let mut out = Vec::new();
    for (label, off) in at {
        let mut buf = vec![0u8; n];
        if f.seek(SeekFrom::Start(off)).is_err() {
            continue;
        }
        let mut filled = 0;
        while filled < n {
            match f.read(&mut buf[filled..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => filled += k,
            }
        }
        buf.truncate(filled);
        let mut mask = masked(&buf, opts.include_text);
        // tokens that are denied (a sample name) are masked too
        if !opts.include_text {
            let text: String = buf
                .iter()
                .map(|b| {
                    if b.is_ascii_graphic() {
                        char::from(*b)
                    } else {
                        ' '
                    }
                })
                .collect();
            let scrubbed = sc.name(&text);
            if scrubbed != text {
                for w in text.split(' ').filter(|w| !w.is_empty()) {
                    if sc.name(w) != w
                        && let Some(i) = text.find(w)
                    {
                        mask[i..i + w.len()].iter_mut().for_each(|m| *m = true);
                    }
                }
            }
        }
        let hex = buf
            .iter()
            .zip(&mask)
            .map(|(b, m)| {
                if *m {
                    "__".to_string()
                } else {
                    format!("{b:02x}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        out.push(HexSample {
            at: label,
            offset: off,
            hex,
        });
    }
    out
}

/// Decode one unit of data to see whether decoding works; returns its shape, never values.
fn first_read(ds: &mut dyn Dataset, info: &FileInfo) -> Result<Option<String>> {
    if let Some(img) = info.images.first() {
        let bytes = u64::from(img.size_x)
            .saturating_mul(u64::from(img.size_y))
            .saturating_mul(img.pixel_type.bytes_per_sample() as u64)
            .saturating_mul(u64::from(img.samples_per_pixel.max(1)));
        let plane = if bytes > FIRST_READ_PLANE_BYTES {
            let region = Region {
                x: 0,
                y: 0,
                width: img.size_x.clamp(1, 512),
                height: img.size_y.clamp(1, 512),
            };
            ds.read_region(0, PlaneIndex::default(), 0, region)?
        } else {
            ds.read_plane(0, PlaneIndex::default())?
        };
        let zero = plane.data.iter().all(|b| *b == 0);
        return Ok(Some(format!(
            "plane {}x{} {} x{}{}{}",
            plane.width,
            plane.height,
            token(&plane.pixel_type),
            plane.samples_per_pixel,
            if bytes > FIRST_READ_PLANE_BYTES {
                " (512x512 region)"
            } else {
                ""
            },
            if zero { ", all zero" } else { "" }
        )));
    }
    if let Some(t) = info.traces.first() {
        let tr = ds.read_trace(t.index, 0, 0, 1024)?;
        let n = tr.channels.first().map_or(0, Vec::len);
        let non_finite = tr
            .channels
            .iter()
            .flatten()
            .filter(|v| !v.is_finite())
            .count();
        return Ok(Some(format!(
            "trace {}: {} channels x {n} samples{}",
            t.index,
            tr.channels.len(),
            if non_finite > 0 {
                format!(", {non_finite} non-finite")
            } else {
                String::new()
            }
        )));
    }
    if let Some(s) = info.spectra.first() {
        if s.scan_count == 0 {
            return Ok(Some(format!("run {}: no spectra", s.index)));
        }
        let sp = ds.read_spectrum(s.index, 0)?;
        return Ok(Some(format!(
            "spectrum 0 of run {}: MS{}, {} points",
            s.index,
            sp.ms_level,
            sp.mz.len()
        )));
    }
    if let Some(t) = info.tables.first() {
        let tb = ds.read_table(t.index, 0, 16)?;
        return Ok(Some(format!(
            "table {}: {} columns x {} rows read",
            t.index,
            tb.columns.len(),
            tb.columns.first().map_or(0, Vec::len)
        )));
    }
    Ok(None)
}

/// Write `bytes` to `path` via a temporary file in the same directory, read it back, compare,
/// then rename it into place (never replaces an existing file unless `overwrite`).
pub fn write_verified(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if path.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            path.display()
        )));
    }
    let name = path
        .file_name()
        .map_or_else(|| "report".into(), |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(&tmp, e))?;
    let back = std::fs::read(&tmp).map_err(|e| Error::io(&tmp, e))?;
    if back != bytes {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Other(format!(
            "{}: read-back differs from what was written",
            tmp.display()
        )));
    }
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
}

/// The default bundle name: `openreadout-report-<12 hex digits>.json`, from a hash of the
/// input's absolute path, so the file name never appears in it and a rerun on the same input
/// finds the same bundle.
pub fn default_name(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let id = hex_string(&Sha256::digest(abs.display().to_string().as_bytes()));
    format!("openreadout-report-{}.json", &id[..12])
}

/// Human-readable rendering: every section, so the user sees what the bundle holds.
pub fn render(out: &ReportOutput) -> String {
    use std::fmt::Write as _;
    let r = &out.report;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "openreadout {} report (bundle version {})",
        r.generator.version, r.report_version
    );
    match &out.output {
        Some(p) => {
            let _ = writeln!(
                s,
                "  wrote {p} ({} bytes). Nothing was sent anywhere.",
                out.bytes.unwrap_or(0)
            );
        }
        None => {
            let _ = writeln!(s, "  dry run: nothing written, nothing sent.");
        }
    }
    let _ = writeln!(
        s,
        "  privacy: {} ({} strings from the file replaced{})",
        r.privacy.mode.replace('_', " "),
        r.privacy.redacted_strings,
        if r.privacy.personal_data.is_empty() {
            String::new()
        } else {
            format!(
                "; personal data found and replaced: {}",
                r.privacy.personal_data.join(", ")
            )
        }
    );
    let i = &r.input;
    let _ = writeln!(
        s,
        "  input: {}, {} bytes{}{}",
        i.kind,
        i.size_bytes,
        i.extension
            .as_deref()
            .map(|e| format!(", .{e}"))
            .unwrap_or_default(),
        i.sha256
            .as_deref()
            .or(i.sha256_first_64mib.as_deref())
            .map(|h| format!(", sha256 {}…", &h[..12.min(h.len())]))
            .unwrap_or_default()
    );
    if let Some(sig) = &i.signature_hex {
        let _ = writeln!(
            s,
            "  signature: {sig}{}",
            i.signature_text
                .as_deref()
                .map(|t| format!("  ({t})"))
                .unwrap_or_default()
        );
    }
    if !i.members.is_empty() {
        let _ = writeln!(
            s,
            "  members: {} files ({})",
            i.members.len() as u64 + i.members_omitted,
            i.member_extensions
                .iter()
                .map(|(e, n)| format!("{n} .{e}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    match &r.detection {
        Some(d) => {
            let _ = writeln!(
                s,
                "  reader: {} ({}, reader confidence {})",
                d.format, d.confidence, d.reader_confidence
            );
        }
        None => {
            let _ = writeln!(
                s,
                "  reader: none claimed it{}",
                if r.candidates.is_empty() {
                    String::new()
                } else {
                    format!(" (readers for this extension: {})", r.candidates.join(", "))
                }
            );
        }
    }
    let _ = writeln!(s, "  decode path:");
    for st in &r.stages {
        let _ = write!(s, "    {:<11} {:<7}", st.stage, st.status);
        if let Some(d) = &st.detail {
            let _ = write!(s, " {d}");
        }
        if let Some(e) = &st.error {
            let _ = write!(s, " [{}] {}", e.code, e.message);
            if let Some(o) = e.offset {
                let _ = write!(s, " (offset {o})");
            }
        }
        s.push('\n');
    }
    if let Some(a) = &r.assurance {
        let _ = writeln!(s, "  assurance: {} - {}", a.level.as_str(), a.summary);
        let _ = writeln!(s, "  fingerprint: {}", a.fingerprint);
    }
    if let Some(st) = &r.structure {
        let _ = writeln!(
            s,
            "  structure: {} entries ({})",
            st.entries_total,
            st.by_kind
                .iter()
                .map(|k| format!("{} {}", k.count, k.kind))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(v) = &r.vendor_keys {
        let _ = writeln!(
            s,
            "  vendor tree: {} key paths (names and types only)",
            v.total_paths
        );
    }
    if !r.samples.is_empty() {
        let _ = writeln!(s, "  hex excerpts: {}", r.samples.len());
    }
    s.push_str("  included: ");
    s.push_str(&r.privacy.included.join("; "));
    s.push_str("\n  left out: ");
    s.push_str(&r.privacy.excluded.join("; "));
    s.push('\n');
    if out.output.is_some() {
        let _ = writeln!(
            s,
            "Review the file, then attach it to a new-variant issue: {}",
            out.issue_url
        );
        let _ = write!(
            s,
            "A vendor export of the same file (OME-TIFF, mzML, CSV) is the ground truth that lets it be validated."
        );
    }
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_replaced_only_whole() {
        assert_eq!(
            replace_words("BF007 and BF0071", "BF007", "<r>"),
            "<r> and BF0071"
        );
        assert_eq!(replace_words("x-BF007.nd2", "BF007", "<r>"), "x-<r>.nd2");
        assert_eq!(
            replace_words("database data", "data", "<r>"),
            "database <r>"
        );
    }

    #[test]
    fn the_path_and_its_specific_components_are_denied() {
        let s = Scrubber::new(Path::new("/Users/jane/lab/mouse7_liver/scan_03.nd2"), false);
        let t = s.text("cannot read /Users/jane/lab/mouse7_liver/scan_03.nd2 at 12");
        assert_eq!(t, "cannot read <input> at 12");
        let t = s.text("block of scan_03 in mouse7_liver");
        assert!(!t.contains("scan_03") && !t.contains("mouse7_liver"), "{t}");
        // generic directory names stay (they are common words in messages)
        assert_eq!(s.text("lab data"), "lab data");
    }

    #[test]
    fn metadata_keeps_numbers_and_vocabulary_and_removes_free_text() {
        let mut s = Scrubber::new(Path::new("f.czi"), false);
        let info = serde_json::json!({
            "format_version": "3.0",
            "images": [{
                "name": "Liver section 4",
                "size_x": 512,
                "pixel_type": "uint16",
                "acquired_at": "2021-03-04T10:00:00Z",
                "channels": [{"name": "Jane's GFP", "exposure_ms": 50.0}],
                "instrument": {"software": "ZEN", "software_version": "3.6"}
            }],
            "experiment": {"instrument": {"kind": {"id": "OBI:0400169", "label": "microscope"}},
                           "sample": {"name": "Liver section 4", "id": "S-17"}}
        });
        let v = s.shape_info(&info, "", "");
        assert_eq!(v["format_version"], "3.0");
        assert_eq!(v["images"][0]["size_x"], 512);
        assert_eq!(v["images"][0]["pixel_type"], "uint16");
        assert_eq!(v["images"][0]["name"], "<text:15>");
        assert_eq!(v["images"][0]["acquired_at"], "<date:2021>");
        assert_eq!(v["images"][0]["channels"][0]["name"], "<text:10>");
        assert_eq!(v["images"][0]["instrument"]["software_version"], "3.6");
        assert_eq!(v["experiment"]["instrument"]["kind"]["id"], "OBI:0400169");
        assert_eq!(v["experiment"]["sample"]["id"], "<text:4>");
        // what was removed from the metadata is removed from every other string
        let t = s.text("image 'Liver section 4' has no plane");
        assert!(
            t.starts_with("image '<redacted-") && !t.contains("Liver"),
            "{t}"
        );
    }

    #[test]
    fn the_panic_hook_finishes_the_bundle_with_the_crash() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("bundle.json");
        let input = dir.path().join("JaneRoe_run.czi");
        let scrubber = Scrubber::new(&input, false);
        set_active(|a| {
            *a = Some(Active {
                snapshot: Report {
                    report_version: REPORT_VERSION.into(),
                    ..Report::default()
                },
                scrubber,
                output: Some(out.clone()),
                overwrite: false,
                stage: "first_read".into(),
            });
        });
        let msg = format!("index out of bounds reading {}", input.display());
        assert_eq!(panic_hook(&msg), Some(out.clone()));
        let r: Report = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let last = r.stages.last().unwrap();
        assert_eq!(
            (last.stage.as_str(), last.status.as_str()),
            ("first_read", "panic")
        );
        let e = last.error.as_ref().unwrap();
        assert_eq!(e.code, "internal_panic");
        assert!(!e.message.contains("JaneRoe"), "{}", e.message);
        // the hook consumes the active report: a second panic writes nothing
        assert_eq!(panic_hook("again"), None);
    }

    #[test]
    fn plain_words_are_replaced_in_names_but_only_quoted_in_prose() {
        let mut s = Scrubber::new(Path::new("f.czi"), false);
        s.deny_str("spectra");
        assert_eq!(s.text("1 runs, 13 spectra"), "1 runs, 13 spectra");
        assert!(s.text("sample 'spectra' unknown").contains("'<redacted-"));
        assert!(s.name("spectra/scan.bin").starts_with("<redacted-"));
        assert_eq!(s.redacted(), 1);
    }

    #[test]
    fn utf16_text_is_masked_and_member_names_keep_only_structure() {
        let b: Vec<u8> = "Jane".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut bytes = vec![0x01, 0x02];
        bytes.extend(&b);
        let m = masked(&bytes, false);
        assert!(!m[0] && !m[1] && m[2..].iter().all(|x| *x), "{m:?}");
        assert_eq!(member_name("acqus"), "acqus");
        assert_eq!(member_name("analysis.tdf"), "analysis.tdf");
        assert_eq!(member_name("1"), "1");
        assert_eq!(member_name("Mouse7_liver.tif"), "Aaaaa9_aaaaa.tif");
        assert_eq!(
            member_name("r01c01f01p01-ch1.tiff"),
            "a99a99a99a99-aa9.tiff"
        );
    }

    #[test]
    fn measurements_are_not_text() {
        for m in [
            "50 ms", "-70.0°C", "17 MHz", "1x1", "2 x 2", "3.3 µs", "300", "12.5%",
        ] {
            assert!(numeric_like(m), "{m}");
        }
        for t in [
            "Custom",
            "Gain 3",
            "mouse 7",
            "7 mice in cage 3",
            "1x1 but not",
        ] {
            assert!(!numeric_like(t), "{t}");
        }
    }

    #[test]
    fn include_text_keeps_names() {
        let mut s = Scrubber::new(Path::new("f.czi"), true);
        let v = s.shape_info(&serde_json::json!({"name": "Liver section 4"}), "", "");
        assert_eq!(v["name"], "Liver section 4");
    }

    #[test]
    fn signature_masks_prose_but_keeps_magic_tokens() {
        let (hex, text) = signature(b"ZISRAWFILE\0\0\0\0\0\0", false);
        assert!(hex.starts_with("5a 49 53"), "{hex}");
        assert_eq!(text.as_deref(), Some("ZISRAWFILE"));
        let (hex, text) = signature(b"Sample: Jane Roe", false);
        assert_eq!(hex, vec!["__"; 16].join(" "));
        assert_eq!(text, None);
    }

    #[test]
    fn vendor_keys_are_a_skeleton_without_values() {
        let s = Scrubber::new(Path::new("f.czi"), false);
        let v = serde_json::json!({
            "Info": {"Operator": "Jane Roe", "Gain": 3, "Channels": [{"Name": "a"}, {"Name": "b"}]},
            "a very long key that is surely free text written by a person, not a vendor field name": 1
        });
        let k = vendor_keys(&s, &v);
        let text = serde_json::to_string(&k).unwrap();
        assert!(!text.contains("Jane"), "{text}");
        assert!(text.contains("Info.Channels[].Name"), "{text}");
        assert!(text.contains("<key:"), "{text}");
        let name = k
            .paths
            .iter()
            .find(|p| p.path == "Info.Channels[].Name")
            .unwrap();
        assert_eq!((name.kind.as_str(), name.count), ("string", 2));
    }
}
