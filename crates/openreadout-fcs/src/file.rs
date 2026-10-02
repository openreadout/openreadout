//! Walking a file: one data set after another via `$NEXTDATA`, reading each HEADER, primary TEXT,
//! supplemental TEXT and ANALYSIS segment, and resolving where each DATA segment is.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::Finding;
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::header::{HEADER_LEN, Header, HeaderError, SegmentRange, VERSIONS, parse_header};
use crate::keywords::{TextParse, is_padded, parse_keywords, parse_uint};
use crate::layout::{ByteOrder, DataType, FieldWidth, Mode, Parameter, read_parameters};

/// Largest keyword segment we read into memory (primary TEXT must sit in the first 99,999,999 bytes anyway).
const MAX_KEYWORD_SEGMENT: u64 = 100_000_000;
/// Stop following `$NEXTDATA` after this many data sets.
const MAX_DATA_SETS: usize = 10_000;

/// Required keywords (FCS 3.1 §3.2.18) that are needed to decode DATA.
const DECODE_KEYWORDS: [&str; 5] = ["$BYTEORD", "$DATATYPE", "$MODE", "$PAR", "$TOT"];
/// Required in FCS 3.x primary TEXT but not needed to decode DATA.
const OTHER_REQUIRED_3X: [&str; 7] = [
    "$BEGINANALYSIS",
    "$BEGINDATA",
    "$BEGINSTEXT",
    "$ENDANALYSIS",
    "$ENDDATA",
    "$ENDSTEXT",
    "$NEXTDATA",
];
/// Keywords whose values are integers and must not be padded (FCS 3.1 §3.2.17).
const NUMERIC_KEYWORDS: [&str; 10] = [
    "$BEGINANALYSIS",
    "$BEGINDATA",
    "$BEGINSTEXT",
    "$ENDANALYSIS",
    "$ENDDATA",
    "$ENDSTEXT",
    "$NEXTDATA",
    "$PAR",
    "$TOT",
    "$TIMESTEP",
];

/// A keyword segment other than the primary TEXT (supplemental TEXT or ANALYSIS).
#[derive(Debug, Clone)]
pub struct AuxSegment {
    /// Absolute byte range.
    pub range: SegmentRange,
    /// Parsed keywords, when the bytes are keyword-value pairs.
    pub keywords: Option<TextParse>,
    /// Why the bytes were not parsed, or what was special about them.
    pub note: Option<String>,
}

/// Which pair of offsets located a DATA segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetSource {
    /// `$BEGINDATA` / `$ENDDATA` in the primary TEXT.
    Text,
    /// HEADER bytes 26–41.
    Header,
}

impl OffsetSource {
    pub fn name(self) -> &'static str {
        match self {
            OffsetSource::Text => "text",
            OffsetSource::Header => "header",
        }
    }
}

/// One data set: HEADER + TEXT (+ supplemental TEXT) + DATA (+ ANALYSIS).
#[derive(Debug, Clone)]
pub struct DataSet {
    /// Zero-based position in the `$NEXTDATA` chain.
    pub index: u32,
    /// Absolute offset of this data set's HEADER.
    pub offset: u64,
    pub header: Header,
    /// First byte of the primary TEXT.
    pub delimiter: u8,
    /// Absolute byte range of the primary TEXT.
    pub text_range: SegmentRange,
    pub text: TextParse,
    pub supplemental: Option<AuxSegment>,
    pub analysis: Option<AuxSegment>,
    pub data_type: Option<DataType>,
    pub byte_order: Option<ByteOrder>,
    pub mode: Option<Mode>,
    /// `$TOT`.
    pub event_count: Option<u64>,
    pub parameters: Vec<Parameter>,
    /// Absolute byte range of DATA, as resolved.
    pub data_range: Option<SegmentRange>,
    pub data_source: Option<OffsetSource>,
    /// `$NEXTDATA` (relative to `offset`); 0 for the last data set.
    pub next_data: u64,
    /// Problems noticed while reading headers; `check` reports them.
    pub findings: Vec<Finding>,
}

impl DataSet {
    /// Keyword from the primary TEXT, else the supplemental TEXT (case-insensitive).
    pub fn keyword(&self, name: &str) -> Option<&str> {
        self.text.keywords.get(name).or_else(|| {
            self.supplemental
                .as_ref()
                .and_then(|s| s.keywords.as_ref())
                .and_then(|k| k.keywords.get(name))
        })
    }

    /// Bytes per event when every parameter has a fixed width.
    pub fn event_width(&self) -> Option<u64> {
        let dt = self.data_type?;
        if self.parameters.is_empty() {
            return None;
        }
        self.parameters
            .iter()
            .map(|p| p.byte_width(p.data_type.unwrap_or(dt)))
            .try_fold(0u64, |acc, w| acc.checked_add(w?))
    }

    /// `$TOT × event width`, the DATA length the keywords promise.
    pub fn expected_data_len(&self) -> Option<u64> {
        self.event_count?.checked_mul(self.event_width()?)
    }

    /// Free-format ASCII (`$PnB/*/`) somewhere in the parameter list.
    pub fn is_free_format(&self) -> bool {
        self.data_type == Some(DataType::Ascii)
            && self
                .parameters
                .iter()
                .any(|p| p.width == Some(FieldWidth::FreeFormat))
    }

    /// End of the last segment of this data set (absolute, inclusive), for CRC placement.
    pub fn last_segment_end(&self) -> u64 {
        let mut end = self.offset + self.header.header_len.saturating_sub(1);
        end = end.max(self.text_range.end);
        for r in [
            self.data_range,
            self.supplemental.as_ref().map(|s| s.range),
            self.analysis.as_ref().map(|s| s.range),
        ]
        .into_iter()
        .flatten()
        {
            end = end.max(r.end);
        }
        for o in &self.header.other {
            if let Some(a) = o.absolute(self.offset) {
                end = end.max(a.end);
            }
        }
        end
    }
}

/// An opened FCS file: its data sets and where the walk stopped.
#[derive(Debug, Clone)]
pub struct FcsFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub data_sets: Vec<DataSet>,
    /// Findings about the chain itself (a `$NEXTDATA` pointing past the end, a loop, ...).
    pub chain_findings: Vec<Finding>,
}

/// Read `len` bytes at `offset`, clamped to the end of the file.
pub fn read_at(
    f: &mut SourceFile,
    path: &Path,
    offset: u64,
    len: u64,
    file_len: u64,
) -> Result<Vec<u8>> {
    Ok(read_block(f, path, offset, len, file_len)?.bytes)
}

fn header_error(offset: u64, e: &HeaderError) -> Error {
    match e {
        HeaderError::NoSignature => Error::corrupt_at(
            FORMAT_ID,
            offset,
            "no FCS version identifier (`FCS2.0`, `FCS3.0`, `FCS3.1`, `FCS3.2`) where a data set should begin",
        ),
        other => Error::corrupt_at(FORMAT_ID, offset, other.to_string()),
    }
}

impl FcsFile {
    pub fn open(path: &Path) -> Result<FcsFile> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<FcsFile> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut data_sets = Vec::new();
        let mut chain_findings = Vec::new();
        let mut offset = 0u64;
        let mut seen = BTreeSet::new();
        loop {
            let index = data_sets.len() as u32;
            match read_data_set(&mut f, path, file_len, offset, index) {
                Ok(ds) => {
                    let next = ds.next_data;
                    data_sets.push(ds);
                    if next == 0 {
                        break;
                    }
                    let Some(n) = offset.checked_add(next) else {
                        chain_findings.push(
                            Finding::error("bad_next_data", "$NEXTDATA overflows").at(offset),
                        );
                        break;
                    };
                    if n >= file_len {
                        chain_findings.push(
                            Finding::error(
                                "truncated",
                                format!("$NEXTDATA of data set {index} points to byte {n}, past the end of the file ({file_len} bytes)"),
                            )
                            .at(offset),
                        );
                        break;
                    }
                    if !seen.insert(n) || n <= offset {
                        chain_findings.push(
                            Finding::error(
                                "bad_next_data",
                                format!("$NEXTDATA of data set {index} points back to byte {n}"),
                            )
                            .at(offset),
                        );
                        break;
                    }
                    if data_sets.len() >= MAX_DATA_SETS {
                        chain_findings.push(Finding::warning(
                            "too_many_data_sets",
                            format!("stopped after {MAX_DATA_SETS} data sets"),
                        ));
                        break;
                    }
                    offset = n;
                }
                Err(e) => {
                    if index == 0 {
                        return Err(e);
                    }
                    chain_findings.push(
                        Finding::error(
                            "bad_data_set",
                            format!("data set {index} at byte {offset}: {e}"),
                        )
                        .at(offset),
                    );
                    break;
                }
            }
        }
        Ok(FcsFile {
            path: path.to_path_buf(),
            file_len,
            data_sets,
            chain_findings,
        })
    }
}

fn read_data_set(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
    offset: u64,
    index: u32,
) -> Result<DataSet> {
    let head = read_at(f, path, offset, 4096, file_len)?;
    let header = parse_header(&head).map_err(|e| header_error(offset, &e))?;
    if !VERSIONS.contains(&header.version.as_str()) {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("version identifier {}", header.version),
            "FCS 2.0, 3.0, 3.1 and 3.2 are supported; FCS 1.0 and FCS 4.0 are not decoded yet.",
        ));
    }
    let mut findings = Vec::new();
    for b in &header.blank_fields {
        findings.push(
            Finding::info(
                "blank_header_offset",
                format!("HEADER field {b} is blank (read as 0)"),
            )
            .at(offset),
        );
    }
    if header.text.is_unset()
        || header.text.end < header.text.begin
        || header.text.begin < HEADER_LEN as u64
    {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            offset + 10,
            format!(
                "HEADER gives an invalid primary TEXT range {}–{}",
                header.text.begin, header.text.end
            ),
        ));
    }
    let text_range = header
        .text
        .absolute(offset)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "TEXT offsets overflow"))?;
    let text_len = text_range.byte_len().unwrap_or(0);
    if text_len > MAX_KEYWORD_SEGMENT {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            offset + 10,
            format!("primary TEXT claims {text_len} bytes"),
        ));
    }
    if text_range.end >= file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            text_range.begin,
            format!(
                "primary TEXT ends at byte {} but the file has {file_len} bytes (truncated)",
                text_range.end
            ),
        ));
    }
    let raw = read_at(f, path, text_range.begin, text_len, file_len)?;
    let delimiter = raw[0];
    if delimiter == 0 || delimiter > 126 {
        findings.push(
            Finding::warning(
                "bad_delimiter",
                format!("TEXT delimiter byte 0x{delimiter:02x} is outside ASCII 1–126"),
            )
            .at(text_range.begin),
        );
    }
    let text = parse_keywords(&raw, delimiter);
    push_text_findings(&mut findings, &text, "primary TEXT", text_range.begin);
    let kw = &text.keywords;

    // Required keywords.
    let fcs32 = header.version == "FCS3.2";
    for k in DECODE_KEYWORDS {
        // FCS 3.2 deprecates $MODE: list mode is the only mode.
        if !kw.contains(k) && (!fcs32 || k != "$MODE") {
            findings.push(
                Finding::error(
                    "missing_keyword",
                    format!("required keyword {k} is missing"),
                )
                .at(text_range.begin),
            );
        }
    }
    if header.version != "FCS2.0" {
        for k in OTHER_REQUIRED_3X {
            // FCS 3.2 made the supplemental TEXT and ANALYSIS offsets optional.
            let optional_in_32 = !matches!(k, "$BEGINDATA" | "$ENDDATA" | "$NEXTDATA");
            if !kw.contains(k) && (!fcs32 || !optional_in_32) {
                findings.push(
                    Finding::warning(
                        "missing_keyword",
                        format!("keyword {k} (required in FCS 3.x) is missing"),
                    )
                    .at(text_range.begin),
                );
            }
        }
    }
    let padded: Vec<&str> = NUMERIC_KEYWORDS
        .iter()
        .copied()
        .filter(|k| kw.get(k).is_some_and(is_padded))
        .collect();
    if !padded.is_empty() {
        findings.push(
            Finding::info(
                "padded_numeric_value",
                format!(
                    "numeric keyword values carry spaces (FCS 3.1 §3.2.17): {}",
                    padded.join(", ")
                ),
            )
            .at(text_range.begin),
        );
    }

    let data_type = kw.get("$DATATYPE").and_then(DataType::from_code);
    if let Some(v) = kw.get("$DATATYPE")
        && data_type.is_none()
    {
        findings.push(Finding::error(
            "bad_keyword",
            format!("$DATATYPE/{v}/ is not I, F, D or A"),
        ));
    }
    let byte_order = kw.get("$BYTEORD").map(ByteOrder::from_value);
    if let Some(ByteOrder::Mixed(s)) = &byte_order {
        findings.push(Finding::warning(
            "mixed_byte_order",
            format!("$BYTEORD/{s}/ is neither 1,2,3,4 nor 4,3,2,1; values cannot be decoded"),
        ));
    }
    let mode = kw
        .get("$MODE")
        .and_then(Mode::from_code)
        .or(if fcs32 && !kw.contains("$MODE") {
            Some(Mode::List)
        } else {
            None
        });
    match (kw.get("$MODE"), mode) {
        (Some(v), None) => findings.push(Finding::error("bad_keyword", format!("$MODE/{v}/ is not L, C or U"))),
        (_, Some(Mode::Correlated | Mode::Uncorrelated)) => findings.push(Finding::warning(
            "histogram_mode",
            "DATA holds histograms ($MODE C or U, deprecated in FCS 3.1); only list mode is decoded",
        )),
        _ => {}
    }
    let event_count = kw.get("$TOT").and_then(parse_uint);
    if kw.contains("$TOT") && event_count.is_none() {
        findings.push(Finding::error("bad_keyword", "$TOT is not an integer"));
    }
    let par = kw.get("$PAR").and_then(parse_uint);
    if kw.contains("$PAR") && par.is_none() {
        findings.push(Finding::error("bad_keyword", "$PAR is not an integer"));
    }
    let par = par.unwrap_or(0).min(100_000) as u32;
    let parameters = read_parameters(kw, par);
    parameter_findings(&mut findings, &parameters, data_type, &header.version);

    let mut ds = DataSet {
        index,
        offset,
        header,
        delimiter,
        text_range,
        text,
        supplemental: None,
        analysis: None,
        data_type,
        byte_order,
        mode,
        event_count,
        parameters,
        data_range: None,
        data_source: None,
        next_data: 0,
        findings,
    };
    resolve_data(&mut ds, file_len);
    ds.supplemental = read_aux(
        f,
        path,
        file_len,
        &mut ds,
        ("$BEGINSTEXT", "$ENDSTEXT"),
        None,
        "supplemental TEXT",
    )?;
    let header_analysis = ds.header.analysis;
    ds.analysis = read_aux(
        f,
        path,
        file_len,
        &mut ds,
        ("$BEGINANALYSIS", "$ENDANALYSIS"),
        Some(header_analysis),
        "ANALYSIS",
    )?;
    ds.next_data = match ds.text.keywords.get("$NEXTDATA") {
        None => 0,
        Some(v) => {
            if let Some(n) = parse_uint(v) {
                n
            } else {
                ds.findings.push(Finding::warning(
                    "bad_keyword",
                    format!("$NEXTDATA/{v}/ is not an integer; treated as 0"),
                ));
                0
            }
        }
    };
    Ok(ds)
}

fn push_text_findings(findings: &mut Vec<Finding>, t: &TextParse, what: &str, at: u64) {
    if t.unterminated {
        findings.push(
            Finding::warning(
                "text_unterminated",
                format!("{what} does not end with the delimiter"),
            )
            .at(at),
        );
    }
    if let Some(k) = &t.dangling_keyword {
        findings.push(
            Finding::warning(
                "dangling_keyword",
                format!("{what} ends with keyword {k:?} that has no value"),
            )
            .at(at),
        );
    }
    if t.empty_values > 0 {
        findings.push(Finding::warning("empty_value", format!("{what} has {} zero-length keywords or values (not permitted by FCS 3.1 §3.2.9)", t.empty_values)).at(at));
    }
    if t.nonprintable_keywords > 0 {
        findings.push(
            Finding::warning(
                "nonprintable_keyword",
                format!(
                    "{what} has {} keywords with bytes outside printable ASCII",
                    t.nonprintable_keywords
                ),
            )
            .at(at),
        );
    }
    if t.latin1_values > 0 {
        findings.push(
            Finding::info(
                "non_utf8_value",
                format!(
                    "{what} has {} values that are not UTF-8; read as Latin-1",
                    t.latin1_values
                ),
            )
            .at(at),
        );
    }
    if !t.keywords.duplicates.is_empty() {
        let mut d = t.keywords.duplicates.clone();
        d.dedup();
        findings.push(
            Finding::warning(
                "duplicate_keyword",
                format!(
                    "{what} repeats keywords (last value wins): {}",
                    d.join(", ")
                ),
            )
            .at(at),
        );
    }
}

fn parameter_findings(
    findings: &mut Vec<Finding>,
    params: &[Parameter],
    dt: Option<DataType>,
    version: &str,
) {
    let mut names = BTreeSet::new();
    let mut dups = BTreeSet::new();
    let mut log_zero = Vec::new();
    for p in params {
        let n = p.number;
        if p.short_name.is_empty() {
            findings.push(Finding::error(
                "missing_keyword",
                format!("required keyword $P{n}N is missing"),
            ));
        } else if !names.insert(p.short_name.clone()) {
            dups.insert(p.short_name.clone());
        }
        if p.short_name.contains(',') {
            findings.push(Finding::warning(
                "bad_keyword",
                format!(
                    "$P{n}N {:?} contains a comma (not allowed in FCS 3.1)",
                    p.short_name
                ),
            ));
        }
        match p.width {
            None => findings.push(Finding::error("missing_keyword", format!("required keyword $P{n}B is missing or not a number"))),
            Some(FieldWidth::FreeFormat) if dt != Some(DataType::Ascii) => {
                findings.push(Finding::error("bad_keyword", format!("$P{n}B/*/ is only valid with $DATATYPE/A/")));
            }
            Some(FieldWidth::Fixed(b)) => match p.data_type.or(dt) {
                Some(DataType::Float) if b != 32 => findings.push(Finding::error("bad_keyword", format!("$P{n}B/{b}/ with $DATATYPE/F/ (must be 32)"))),
                Some(DataType::Double) if b != 64 => findings.push(Finding::error("bad_keyword", format!("$P{n}B/{b}/ with $DATATYPE/D/ (must be 64)"))),
                Some(DataType::Integer) if b == 0 || b % 8 != 0 || b > 64 => findings.push(Finding::error(
                    "unsupported_width",
                    format!("$P{n}B/{b}/: integer fields that are not whole bytes (or wider than 64 bits) are not decoded"),
                )),
                _ => {}
            },
            _ => {}
        }
        if p.range_text.is_none() {
            findings.push(Finding::warning(
                "missing_keyword",
                format!("required keyword $P{n}R is missing"),
            ));
        } else if p.range.is_none() {
            findings.push(Finding::warning(
                "bad_keyword",
                format!(
                    "$P{n}R/{}/ is not a number",
                    p.range_text.as_deref().unwrap_or("")
                ),
            ));
        }
        if version != "FCS2.0" && p.amplification.is_none() {
            findings.push(Finding::warning(
                "missing_keyword",
                format!("keyword $P{n}E (required in FCS 3.x) is missing or malformed"),
            ));
        }
        if let Some([d, o]) = p.amplification
            && d > 0.0
            && o == 0.0
        {
            log_zero.push(format!("$P{n}E/{d},0/"));
        }
    }
    if !log_zero.is_empty() {
        findings.push(Finding::info(
            "log_zero_offset",
            format!(
                "{} use a zero log offset, invalid per FCS 3.1 (readers treat f2 = 0 as 1)",
                log_zero.join(", ")
            ),
        ));
    }
    if !dups.is_empty() {
        findings.push(Finding::warning(
            "duplicate_parameter_name",
            format!(
                "$PnN names are not unique: {}",
                dups.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
}

/// Pick the DATA offsets: TEXT keywords first for FCS 3.x, HEADER first for FCS 2.0; a pair whose
/// length matches `$TOT × event width` (or is one byte longer, a common writer slip) wins.
fn resolve_data(ds: &mut DataSet, file_len: u64) {
    let kw = &ds.text.keywords;
    let text_pair = match (
        kw.get("$BEGINDATA").and_then(parse_uint),
        kw.get("$ENDDATA").and_then(parse_uint),
    ) {
        (Some(b), Some(e)) if !(b == 0 && e == 0) => Some(SegmentRange::new(b, e)),
        _ => None,
    };
    let header_pair = (!ds.header.data.is_unset()).then_some(ds.header.data);
    let mut candidates: Vec<(OffsetSource, SegmentRange)> = Vec::new();
    let text_first = ds.header.version != "FCS2.0";
    let ordered = if text_first {
        [
            (OffsetSource::Text, text_pair),
            (OffsetSource::Header, header_pair),
        ]
    } else {
        [
            (OffsetSource::Header, header_pair),
            (OffsetSource::Text, text_pair),
        ]
    };
    for (src, r) in ordered {
        if let Some(r) = r
            && r.end >= r.begin
        {
            candidates.push((src, r));
        }
    }
    let expected = ds.expected_data_len();
    if let (Some(t), Some(h)) = (text_pair, header_pair)
        && t != h
    {
        ds.findings.push(
            Finding::warning(
                "offset_discrepancy",
                format!(
                    "HEADER says DATA is {}–{}, TEXT ($BEGINDATA/$ENDDATA) says {}–{}",
                    h.begin, h.end, t.begin, t.end
                ),
            )
            .at(ds.offset + 26),
        );
    }
    let fits = |r: &SegmentRange| {
        let len = r.byte_len().unwrap_or(0);
        expected.is_none_or(|e| len == e || len == e + 1)
    };
    let chosen = candidates
        .iter()
        .find(|(_, r)| fits(r))
        .or_else(|| candidates.first())
        .copied();
    let Some((src, rel)) = chosen else {
        if ds.event_count.unwrap_or(0) > 0 {
            ds.findings.push(
                Finding::error(
                    "missing_data_offsets",
                    "neither the HEADER nor $BEGINDATA/$ENDDATA locate the DATA segment",
                )
                .at(ds.offset + 26),
            );
        }
        return;
    };
    let Some(abs) = rel.absolute(ds.offset) else {
        ds.findings
            .push(Finding::error("bad_offset", "DATA offsets overflow"));
        return;
    };
    ds.data_range = Some(abs);
    ds.data_source = Some(src);
    let len = abs.byte_len().unwrap_or(0);
    if let Some(e) = expected {
        if len == e + 1 {
            ds.findings.push(Finding::warning("data_end_off_by_one", format!("DATA is {len} bytes, one more than $TOT × event width ({e}); the end offset likely points one byte past the last byte")).at(abs.begin));
        } else if len > e {
            ds.findings.push(Finding::warning("data_length_mismatch", format!("DATA is {len} bytes but $TOT × event width is {e}; the extra bytes are ignored")).at(abs.begin));
        } else if len < e {
            ds.findings.push(
                Finding::error(
                    "data_length_mismatch",
                    format!("DATA is {len} bytes but $TOT × event width needs {e}"),
                )
                .at(abs.begin),
            );
        }
    }
    let need_end = match expected {
        Some(e) if e > 0 => abs.begin.saturating_add(e) - 1,
        Some(_) => abs.begin,
        None => abs.end,
    };
    if need_end >= file_len && expected != Some(0) {
        ds.findings.push(
            Finding::error(
                "truncated",
                format!(
                    "DATA needs bytes up to {need_end} but the file has {file_len} bytes; {} bytes are missing",
                    need_end + 1 - file_len
                ),
            )
            .at(abs.begin),
        );
    }
}

fn read_aux(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
    ds: &mut DataSet,
    (begin_kw, end_kw): (&str, &str),
    header_range: Option<SegmentRange>,
    what: &str,
) -> Result<Option<AuxSegment>> {
    let kw = &ds.text.keywords;
    let from_text = match (
        kw.get(begin_kw).and_then(parse_uint),
        kw.get(end_kw).and_then(parse_uint),
    ) {
        (Some(b), Some(e)) if !(b == 0 && e == 0) => Some(SegmentRange::new(b, e)),
        _ => None,
    };
    let rel = match (from_text, header_range.filter(|r| !r.is_unset())) {
        (Some(t), _) => t,
        (None, Some(h)) => h,
        (None, None) => return Ok(None),
    };
    let Some(range) = rel.absolute(ds.offset) else {
        ds.findings.push(Finding::error(
            "bad_offset",
            format!("{what} offsets overflow"),
        ));
        return Ok(None);
    };
    let Some(len) = range.byte_len() else {
        ds.findings.push(Finding::warning(
            "bad_offset",
            format!(
                "{what} range {}–{} ends before it begins",
                rel.begin, rel.end
            ),
        ));
        return Ok(None);
    };
    if range == ds.text_range {
        return Ok(Some(AuxSegment {
            range,
            keywords: None,
            note: Some("points at the primary TEXT segment".into()),
        }));
    }
    if range.end >= file_len {
        ds.findings.push(
            Finding::error(
                "truncated",
                format!(
                    "{what} ends at byte {} but the file has {file_len} bytes",
                    range.end
                ),
            )
            .at(range.begin),
        );
        return Ok(Some(AuxSegment {
            range,
            keywords: None,
            note: Some("extends past the end of the file".into()),
        }));
    }
    if len > MAX_KEYWORD_SEGMENT {
        return Ok(Some(AuxSegment {
            range,
            keywords: None,
            note: Some(format!("{len} bytes; too large to parse")),
        }));
    }
    let raw = read_at(f, path, range.begin, len, file_len)?;
    if raw.starts_with(b"PK\x03\x04") {
        ds.findings.push(
            Finding::info(
                "non_keyword_segment",
                format!("{what} holds a ZIP archive, not keyword-value pairs"),
            )
            .at(range.begin),
        );
        return Ok(Some(AuxSegment {
            range,
            keywords: None,
            note: Some(
                "holds a ZIP archive (signature PK\\x03\\x04), not keyword-value pairs".into(),
            ),
        }));
    }
    let parsed = parse_keywords(&raw, ds.delimiter);
    if parsed.nonprintable_keywords > 0 && parsed.nonprintable_keywords * 2 > parsed.keywords.len()
    {
        ds.findings.push(
            Finding::warning(
                "non_keyword_segment",
                format!("{what} does not parse as keyword-value pairs"),
            )
            .at(range.begin),
        );
        return Ok(Some(AuxSegment {
            range,
            keywords: None,
            note: Some("does not parse as keyword-value pairs".into()),
        }));
    }
    push_text_findings(&mut ds.findings, &parsed, what, range.begin);
    Ok(Some(AuxSegment {
        range,
        keywords: Some(parsed),
        note: None,
    }))
}
