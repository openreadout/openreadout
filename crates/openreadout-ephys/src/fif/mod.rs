//! FIF files (Functional Image File format): the MEG and EEG recordings of MEGIN / Elekta
//! Neuromag systems and the files MNE-Python and MNE-C write. A FIF file is a sequence of tags
//! (16-byte big-endian header: kind, type, size, next; then the data), nested in blocks by
//! block-start and block-end tags. The measurement block holds the measurement info (channels,
//! sample rate, date) and the raw data block holds the samples as data buffers of
//! samples × channels. Notes and vocabulary: `docs/formats/fif.md`; provenance:
//! `docs/provenance/fif.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{latin1, read_block};
use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry,
    SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

/// Format id of FIF files.
pub const FIF_FORMAT_ID: &str = "fif";
/// Bytes of a tag header (kind, type, size, next).
pub const FIF_TAG_HEADER: u64 = 16;
/// Tags accepted at most in one file.
pub const MAX_FIF_TAGS: usize = 20_000_000;
/// Block nesting accepted at most.
pub const MAX_FIF_DEPTH: usize = 64;
/// Channels accepted at most.
pub const MAX_FIF_CHANNELS: usize = 65_536;
/// Bytes of one string or small tag read at most.
pub const MAX_FIF_TAG_READ: u64 = 16 << 20;
/// Split-file parts followed at most.
pub const MAX_FIF_PARTS: usize = 256;
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_FIF_READ: u64 = 1 << 24;
/// Bytes of data buffers read per `read_trace` call at most (one buffer is always read).
pub const MAX_FIF_READ_BYTES: u64 = 64 << 20;

// Tag kinds.
const K_FILE_ID: i32 = 100;
const K_DIR_POINTER: i32 = 101;
const K_BLOCK_START: i32 = 104;
const K_BLOCK_END: i32 = 105;
const K_CREATOR: i32 = 113;
const K_REF_ROLE: i32 = 115;
const K_REF_FILE_NUM: i32 = 117;
const K_REF_FILE_NAME: i32 = 118;
const K_UTC_OFFSET: i32 = 159;
const K_NCHAN: i32 = 200;
const K_SFREQ: i32 = 201;
const K_CH_INFO: i32 = 203;
const K_MEAS_DATE: i32 = 204;
const K_COMMENT: i32 = 206;
const K_FIRST_SAMPLE: i32 = 208;
const K_EXPERIMENTER: i32 = 212;
const K_LOWPASS: i32 = 219;
const K_HIGHPASS: i32 = 223;
const K_LINE_FREQ: i32 = 235;
const K_CH_SCAN_NO: i32 = 250;
const K_CH_LOGICAL_NO: i32 = 251;
const K_CH_KIND: i32 = 252;
const K_CH_RANGE: i32 = 253;
const K_CH_CAL: i32 = 254;
const K_CH_UNIT: i32 = 256;
const K_CH_UNIT_MUL: i32 = 257;
const K_CH_DACQ_NAME: i32 = 258;
const K_CH_COIL_TYPE: i32 = 350;
const K_DATA_BUFFER: i32 = 300;
const K_DATA_SKIP: i32 = 301;
const K_PROJ_NAME: i32 = 501;
const K_MNE_CH_NAME_LIST: i32 = 3507;
const K_MNE_COMMAND_LINE: i32 = 3551;
const K_ANNOT_ONSET: i32 = 3568;
const K_ANNOT_END: i32 = 3569;

// Block kinds.
const B_ROOT: i32 = 999;
const B_MEAS: i32 = 100;
const B_MEAS_INFO: i32 = 101;
const B_RAW: i32 = 102;
const B_PROCESSED: i32 = 103;
const B_EVOKED: i32 = 104;
const B_CONTINUOUS: i32 = 112;
const B_CH_INFO: i32 = 113;
const B_REF: i32 = 118;
const B_IAS_RAW: i32 = 119;
const B_MNE_BAD_CHANNELS: i32 = 359;
const B_MNE_EPOCHS: i32 = 373;
const B_PROCESSING_HISTORY: i32 = 900;
const B_ANNOTATIONS: i32 = 3810;

// Data types.
const T_SHORT: u32 = 2;
const T_INT: u32 = 3;
const T_FLOAT: u32 = 4;
const T_DOUBLE: u32 = 5;
const T_DAU_PACK16: u32 = 16;
const T_COMPLEX_FLOAT: u32 = 20;
const T_COMPLEX_DOUBLE: u32 = 21;
const T_CH_INFO_STRUCT: u32 = 30;
const T_ID_STRUCT: u32 = 31;
const T_DIR_ENTRY_STRUCT: u32 = 32;

/// The meas-date value FIF writers use for "no date" (seconds 0, microseconds 2³¹ − 1).
const DATE_NONE_USECS: i32 = i32::MAX;

/// One tag: its header and where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FifTag {
    /// Tag kind (what the data mean).
    pub kind: i32,
    /// Data type code.
    pub data_type: u32,
    /// Bytes of data after the header.
    pub size: u32,
    /// File offset of the header; the data start 16 bytes later.
    pub pos: u64,
}

impl FifTag {
    /// File offset of the data.
    pub fn data_pos(&self) -> u64 {
        self.pos + FIF_TAG_HEADER
    }
}

/// A block: its kind, the tags directly inside it (in file order) and its child blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FifBlock {
    /// Block kind (999 for the file's root).
    pub kind: i32,
    /// Tags directly inside the block, block-start and block-end tags left out.
    pub tags: Vec<FifTag>,
    /// Child blocks in file order.
    pub children: Vec<FifBlock>,
}

impl FifBlock {
    /// Every block of `kind` at or below this one, depth first.
    pub fn find(&self, kind: i32) -> Vec<&FifBlock> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(b) = stack.pop() {
            if b.kind == kind {
                out.push(b);
            }
            stack.extend(b.children.iter().rev());
        }
        out
    }

    /// The first tag of `kind` directly inside the block.
    pub fn tag(&self, kind: i32) -> Option<&FifTag> {
        self.tags.iter().find(|t| t.kind == kind)
    }
}

/// One channel of the measurement info.
#[derive(Debug, Clone, PartialEq)]
pub struct FifChannel {
    /// Scan number (position in the acquisition order).
    pub scan_number: i32,
    /// Logical channel number.
    pub logical_number: i32,
    /// Channel kind code (1 MEG, 2 EEG, 3 stimulus, …; `kind_name`).
    pub kind: i32,
    /// Range: recorded number to the voltage at the output.
    pub range: f32,
    /// Calibration: output voltage to the channel's unit.
    pub cal: f32,
    /// Coil type code (the sensor geometry of MEG channels).
    pub coil_type: i32,
    /// Sensor location and orientation (12 values).
    pub loc: [f32; 12],
    /// Unit code (107 V, 112 T, 201 T/m, …; `unit_name`).
    pub unit: i32,
    /// Unit multiplier exponent (0 for SI units).
    pub unit_mul: i32,
    /// Channel name (the extended name when the file has one).
    pub name: String,
}

impl FifChannel {
    /// The factor from stored numbers to channel units: range × cal, in float64.
    pub fn factor(&self) -> f64 {
        f64::from(self.range) * f64::from(self.cal)
    }
}

/// The name of a channel kind code, in our vocabulary.
pub fn kind_name(kind: i32) -> Option<&'static str> {
    Some(match kind {
        1 => "meg",
        2 => "eeg",
        3 => "stimulus",
        102 => "bio",
        201 => "mcg",
        202 => "eog",
        301 => "reference_meg",
        302 => "emg",
        402 => "ecg",
        502 => "misc",
        602 => "respiration",
        802 => "seeg",
        803 => "dbs",
        900 => "system",
        902 => "ecog",
        910 => "active_shielding",
        920 => "flux_excitation",
        1000 => "dipole",
        1001 => "goodness_of_fit",
        1100 => "fnirs",
        1200 => "temperature",
        1300 => "galvanic",
        1400 => "eyetrack",
        _ => return None,
    })
}

/// The symbol of a unit code (SI), `""` for unitless.
pub fn unit_name(unit: i32) -> Option<&'static str> {
    Some(match unit {
        -1 | 0 => "",
        1 => "m",
        2 => "kg",
        3 => "s",
        4 => "A",
        5 => "K",
        6 => "mol",
        7 => "rad",
        8 => "sr",
        9 => "cd",
        10 => "mol/m^3",
        101 => "Hz",
        102 => "N",
        103 => "Pa",
        104 => "J",
        105 => "W",
        106 => "C",
        107 => "V",
        108 => "F",
        109 => "Ohm",
        110 => "S",
        111 => "Wb",
        112 => "T",
        113 => "H",
        114 => "Cel",
        115 => "lm",
        116 => "lx",
        117 => "V/m^2",
        118 => "s^2",
        201 => "T/m",
        202 => "Am",
        203 => "Am/m^2",
        204 => "Am/m^3",
        210 => "px",
        _ => return None,
    })
}

/// The stored sample type of a data buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FifSampleType {
    /// 16-bit integers (`short` and the 16-bit DAU packing).
    Int16,
    /// 32-bit integers.
    Int32,
    /// 32-bit floats.
    Float32,
    /// 64-bit floats.
    Float64,
}

impl FifSampleType {
    /// The type of a data-type code, if data buffers of it are read.
    pub fn of(code: u32) -> Option<Self> {
        match code {
            T_SHORT | T_DAU_PACK16 => Some(Self::Int16),
            T_INT => Some(Self::Int32),
            T_FLOAT => Some(Self::Float32),
            T_DOUBLE => Some(Self::Float64),
            _ => None,
        }
    }

    /// Bytes per sample.
    pub fn bytes(self) -> u64 {
        match self {
            Self::Int16 => 2,
            Self::Int32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    /// NumPy-style name.
    pub fn dtype(self) -> &'static str {
        match self {
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
        }
    }
}

/// One data buffer of the raw data: `samples` × channels values, sample by sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FifBuffer {
    /// Index of the split-file part the buffer is in.
    pub part: usize,
    /// File offset of the samples.
    pub data_pos: u64,
    /// Stored sample type.
    pub sample_type: FifSampleType,
    /// Samples per channel.
    pub samples: u64,
    /// Sample number of the first sample (counted from the acquisition start, as the
    /// `first sample` tag counts).
    pub first: u64,
}

/// A run of data buffers without a skip between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FifSweep {
    /// Index of the first buffer in `FifFile::buffers`.
    pub first_buffer: usize,
    /// Index after the last buffer.
    pub end_buffer: usize,
    /// Sample number of the first sample.
    pub first: u64,
    /// Samples per channel.
    pub samples: u64,
}

/// One annotation of an MNE annotation block.
#[derive(Debug, Clone, PartialEq)]
pub struct FifAnnotation {
    /// Seconds after the first sample.
    pub onset_s: f64,
    /// Seconds.
    pub duration_s: f64,
    /// Description.
    pub text: String,
}

/// The kind of block the samples are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FifDataBlock {
    /// Raw data (block 102).
    Raw,
    /// Continuous data (block 112).
    Continuous,
    /// Raw data recorded with internal active shielding (block 119).
    ActiveShieldingRaw,
}

impl FifDataBlock {
    /// Our name of the block kind.
    pub fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Continuous => "continuous",
            Self::ActiveShieldingRaw => "active shielding raw",
        }
    }
}

/// The parsed file (and the parts it continues into).
#[derive(Debug, Clone)]
pub struct FifFile {
    /// File length of the first part.
    pub file_len: u64,
    /// FIFF version of the file id (major × 65536 + minor).
    pub version: i32,
    /// Seconds of the file id time stamp.
    pub file_id_secs: i32,
    /// The block tree.
    pub root: FifBlock,
    /// The tag directory came from the directory tag (not a scan).
    pub from_directory: bool,
    /// Number of tags.
    pub tag_count: usize,
    /// Channels in data order.
    pub channels: Vec<FifChannel>,
    /// Samples per second.
    pub sfreq: f64,
    /// Low-pass corner, Hz.
    pub lowpass_hz: Option<f64>,
    /// High-pass corner, Hz.
    pub highpass_hz: Option<f64>,
    /// Line frequency, Hz.
    pub line_freq_hz: Option<f64>,
    /// Measurement date: seconds and microseconds since 1970 (UTC).
    pub meas_date: Option<(i32, i32)>,
    /// UTC offset text.
    pub utc_offset: Option<String>,
    /// Experimenter.
    pub experimenter: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Project name.
    pub project: Option<String>,
    /// Names of bad channels.
    pub bad_channels: Vec<String>,
    /// Creator strings found in the file (acquisition and processing programs).
    pub creators: Vec<String>,
    /// MNE command line, when the file has one.
    pub command_line: Option<String>,
    /// Which block holds the samples.
    pub data_block: Option<FifDataBlock>,
    /// The other kinds of content found (epochs, evoked responses, …) when there are no raw data.
    pub other_content: Vec<&'static str>,
    /// Data buffers of every part, in order.
    pub buffers: Vec<FifBuffer>,
    /// Runs of contiguous buffers.
    pub sweeps: Vec<FifSweep>,
    /// Sample number of the first sample.
    pub first_sample: u64,
    /// Paths of the parts (the first is the file itself).
    pub parts: Vec<PathBuf>,
    /// Lengths of the parts.
    pub part_lens: Vec<u64>,
    /// Annotations.
    pub annotations: Vec<FifAnnotation>,
    /// Structural problems found while parsing.
    pub findings: Vec<Finding>,
}

/// The file starts with a file-id tag: kind 100, type 31 (id struct), 20 bytes.
pub fn looks_like_fif(head: &[u8]) -> bool {
    head.get(..12) == Some(&[0, 0, 0, 100, 0, 0, 0, 31, 0, 0, 0, 20][..])
}

fn be_i32(b: &[u8], at: usize) -> i32 {
    b.get(at..at + 4)
        .map_or(0, |x| i32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

fn be_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(be_i32(b, at) as u32)
}

/// A tag's header at `pos`, and its `next` field.
fn read_header(
    f: &mut SourceFile,
    path: &Path,
    pos: u64,
    file_len: u64,
) -> Result<Option<(FifTag, i32)>> {
    if pos.checked_add(FIF_TAG_HEADER).is_none_or(|e| e > file_len) {
        return Ok(None);
    }
    let b = read_block(f, path, pos, FIF_TAG_HEADER, file_len)?;
    if b.bytes.len() < 16 {
        return Ok(None);
    }
    let size = be_i32(&b.bytes, 8);
    let Ok(size) = u32::try_from(size) else {
        return Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            pos,
            format!(
                "tag of kind {} has a negative size {size}",
                be_i32(&b.bytes, 0)
            ),
        ));
    };
    Ok(Some((
        FifTag {
            kind: be_i32(&b.bytes, 0),
            data_type: be_i32(&b.bytes, 4) as u32,
            size,
            pos,
        },
        be_i32(&b.bytes, 12),
    )))
}

/// The data of a small tag (at most `MAX_FIF_TAG_READ` bytes).
fn tag_data(f: &mut SourceFile, path: &Path, tag: &FifTag, file_len: u64) -> Result<Vec<u8>> {
    let n = u64::from(tag.size);
    if n > MAX_FIF_TAG_READ {
        return Err(Error::unsupported(
            FIF_FORMAT_ID,
            format!("a tag of kind {} with {n} bytes of metadata", tag.kind),
            "The file may be damaged; the limits are in docs/formats/fif.md.",
        ));
    }
    let b = read_block(f, path, tag.data_pos(), n, file_len)?;
    if (b.bytes.len() as u64) < n {
        return Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            tag.pos,
            format!("tag of kind {} is cut off by the end of the file", tag.kind),
        ));
    }
    Ok(b.bytes)
}

fn tag_int(f: &mut SourceFile, path: &Path, tag: &FifTag, len: u64) -> Result<i32> {
    let d = tag_data(f, path, tag, len)?;
    if d.len() < 4 {
        return Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            tag.pos,
            format!(
                "tag of kind {} holds {} bytes, not an integer",
                tag.kind,
                d.len()
            ),
        ));
    }
    Ok(be_i32(&d, 0))
}

fn tag_float(f: &mut SourceFile, path: &Path, tag: &FifTag, len: u64) -> Result<f64> {
    let d = tag_data(f, path, tag, len)?;
    match (tag.data_type, d.len()) {
        (T_FLOAT, 4..) => Ok(f64::from(be_f32(&d, 0))),
        (T_DOUBLE, 8..) => Ok(f64::from_be_bytes([
            d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
        ])),
        _ => Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            tag.pos,
            format!(
                "tag of kind {} has type {} and {} bytes, not a number",
                tag.kind,
                tag.data_type,
                d.len()
            ),
        )),
    }
}

fn tag_floats(f: &mut SourceFile, path: &Path, tag: &FifTag, len: u64) -> Result<Vec<f64>> {
    let d = tag_data(f, path, tag, len)?;
    Ok(match tag.data_type {
        T_FLOAT => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|x| f64::from(f32::from_be_bytes(*x)))
            .collect(),
        T_DOUBLE => d
            .as_chunks::<8>()
            .0
            .iter()
            .map(|x| f64::from_be_bytes(*x))
            .collect(),
        _ => Vec::new(),
    })
}

fn tag_text(f: &mut SourceFile, path: &Path, tag: &FifTag, len: u64) -> Result<String> {
    let d = tag_data(f, path, tag, len)?;
    let end = d.iter().position(|&c| c == 0).unwrap_or(d.len());
    Ok(latin1(&d[..end]))
}

/// The tag directory: from the directory tag the file points to, else by following the tags.
fn directory(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
    findings: &mut Vec<Finding>,
) -> Result<(Vec<FifTag>, bool)> {
    let Some((first, next)) = read_header(f, path, 0, file_len)? else {
        return Err(Error::corrupt(
            FIF_FORMAT_ID,
            "the file is shorter than one tag",
        ));
    };
    if first.kind != K_FILE_ID || first.data_type != T_ID_STRUCT || first.size != 20 {
        return Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            0,
            "the file does not start with a FIF file-id tag",
        ));
    }
    let second_at = if next > 0 {
        u64::from(next.unsigned_abs())
    } else {
        FIF_TAG_HEADER + 20
    };
    let pointer = read_header(f, path, second_at, file_len)?;
    let dir_pos = match pointer {
        Some((t, _)) if t.kind == K_DIR_POINTER && t.size >= 4 => tag_int(f, path, &t, file_len)?,
        _ => {
            return Err(Error::corrupt_at(
                FIF_FORMAT_ID,
                second_at,
                "the file-id tag is not followed by a directory pointer",
            ));
        }
    };
    if dir_pos > 0 {
        let dp = u64::from(dir_pos.unsigned_abs());
        match read_directory_tag(f, path, dp, file_len)? {
            Some(dir) => return Ok((dir, true)),
            None => findings.push(Finding::warning(
                "directory_unusable",
                format!(
                    "the tag directory the file points to (byte {dp}) cannot be read; the tags were followed one by one instead"
                ),
            )),
        }
    }
    let mut out = Vec::new();
    let mut pos = 0u64;
    let mut seen = std::collections::HashSet::new();
    seen.insert(0u64);
    loop {
        let Some((tag, next)) = read_header(f, path, pos, file_len)? else {
            if pos < file_len {
                findings.push(Finding::warning(
                    "trailing_bytes",
                    format!("{} byte(s) after the last whole tag header", file_len - pos),
                ));
            }
            break;
        };
        out.push(tag);
        if out.len() > MAX_FIF_TAGS {
            return Err(Error::unsupported(
                FIF_FORMAT_ID,
                format!("more than {MAX_FIF_TAGS} tags"),
                "The file may be damaged; the limits are in docs/formats/fif.md.",
            ));
        }
        let end = tag.data_pos() + u64::from(tag.size);
        if end > file_len {
            findings.push(Finding::error(
                "truncated",
                format!(
                    "the tag of kind {} at byte {pos} runs past the end of the file ({end} > {file_len})",
                    tag.kind
                ),
            ));
            break;
        }
        pos = match next {
            0 => end,
            n if n > 0 => u64::from(n.unsigned_abs()),
            _ => break,
        };
        // `next` may point backwards (blocks written later are linked in); a position seen
        // before would loop
        if !seen.insert(pos) {
            findings.push(Finding::error(
                "bad_next",
                format!("the tag chain returns to byte {pos}; the scan stops there"),
            ));
            break;
        }
    }
    Ok((out, false))
}

fn read_directory_tag(
    f: &mut SourceFile,
    path: &Path,
    dp: u64,
    file_len: u64,
) -> Result<Option<Vec<FifTag>>> {
    let Some((t, _)) = read_header(f, path, dp, file_len)? else {
        return Ok(None);
    };
    if t.data_type != T_DIR_ENTRY_STRUCT
        || !t.size.is_multiple_of(16)
        || t.data_pos() + u64::from(t.size) > file_len
    {
        return Ok(None);
    }
    let n = (t.size / 16) as usize;
    if n > MAX_FIF_TAGS {
        return Ok(None);
    }
    let b = read_block(f, path, t.data_pos(), u64::from(t.size), file_len)?;
    let mut out = Vec::with_capacity(n);
    for e in b.bytes.as_chunks::<16>().0 {
        let size = be_i32(e, 8);
        let pos = be_i32(e, 12);
        let (Ok(size), Ok(pos)) = (u32::try_from(size), u64::try_from(pos)) else {
            // writers end the directory with an entry of −1s
            if be_i32(e, 0) == -1 {
                continue;
            }
            return Ok(None);
        };
        if pos + FIF_TAG_HEADER + u64::from(size) > file_len {
            return Ok(None);
        }
        out.push(FifTag {
            kind: be_i32(e, 0),
            data_type: be_i32(e, 4) as u32,
            size,
            pos,
        });
    }
    Ok(Some(out))
}

/// Nest the directory into blocks.
fn tree(
    f: &mut SourceFile,
    path: &Path,
    tags: &[FifTag],
    file_len: u64,
    findings: &mut Vec<Finding>,
) -> Result<FifBlock> {
    let mut stack = vec![FifBlock {
        kind: B_ROOT,
        ..FifBlock::default()
    }];
    for t in tags {
        match t.kind {
            K_BLOCK_START => {
                let kind = tag_int(f, path, t, file_len)?;
                if stack.len() > MAX_FIF_DEPTH {
                    return Err(Error::unsupported(
                        FIF_FORMAT_ID,
                        format!("blocks nested more than {MAX_FIF_DEPTH} deep"),
                        "The file may be damaged; the limits are in docs/formats/fif.md.",
                    ));
                }
                stack.push(FifBlock {
                    kind,
                    ..FifBlock::default()
                });
            }
            K_BLOCK_END => {
                if stack.len() == 1 {
                    findings.push(Finding::warning(
                        "unbalanced_blocks",
                        format!("a block end at byte {} closes no block", t.pos),
                    ));
                    continue;
                }
                let done = stack.pop().expect("more than the root");
                stack.last_mut().expect("root").children.push(done);
            }
            _ => stack.last_mut().expect("root").tags.push(*t),
        }
    }
    if stack.len() > 1 {
        findings.push(Finding::warning(
            "unbalanced_blocks",
            format!(
                "{} block(s) are not closed before the end of the file",
                stack.len() - 1
            ),
        ));
        while stack.len() > 1 {
            let done = stack.pop().expect("more than the root");
            stack.last_mut().expect("root").children.push(done);
        }
    }
    Ok(stack.pop().expect("root"))
}

fn channel_struct(d: &[u8], at: u64) -> Result<FifChannel> {
    if d.len() < 96 {
        return Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            at,
            format!("a channel info tag holds {} bytes, not 96", d.len()),
        ));
    }
    let mut loc = [0f32; 12];
    for (k, v) in loc.iter_mut().enumerate() {
        *v = be_f32(d, 24 + 4 * k);
    }
    let name = &d[80..96];
    let end = name.iter().position(|&c| c == 0).unwrap_or(16);
    Ok(FifChannel {
        scan_number: be_i32(d, 0),
        logical_number: be_i32(d, 4),
        kind: be_i32(d, 8),
        range: be_f32(d, 12),
        cal: be_f32(d, 16),
        coil_type: be_i32(d, 20),
        loc,
        unit: be_i32(d, 72),
        unit_mul: be_i32(d, 76),
        name: String::from_utf8_lossy(&name[..end]).into_owned(),
    })
}

/// What the measurement info says.
#[derive(Default)]
struct MeasInfo {
    nchan: Option<i32>,
    sfreq: Option<f64>,
    channels: Vec<FifChannel>,
    lowpass: Option<f64>,
    highpass: Option<f64>,
    line_freq: Option<f64>,
    meas_date: Option<(i32, i32)>,
    utc_offset: Option<String>,
    experimenter: Option<String>,
    description: Option<String>,
    project: Option<String>,
    bads: Vec<String>,
}

fn meas_info(f: &mut SourceFile, path: &Path, info: &FifBlock, len: u64) -> Result<MeasInfo> {
    let mut m = MeasInfo::default();
    for t in &info.tags {
        match t.kind {
            K_NCHAN => m.nchan = Some(tag_int(f, path, t, len)?),
            K_SFREQ => m.sfreq = Some(tag_float(f, path, t, len)?),
            K_CH_INFO if t.data_type == T_CH_INFO_STRUCT => {
                if m.channels.len() >= MAX_FIF_CHANNELS {
                    return Err(Error::unsupported(
                        FIF_FORMAT_ID,
                        format!("more than {MAX_FIF_CHANNELS} channels"),
                        "The file may be damaged; the limits are in docs/formats/fif.md.",
                    ));
                }
                let d = tag_data(f, path, t, len)?;
                m.channels.push(channel_struct(&d, t.pos)?);
            }
            K_LOWPASS => m.lowpass = Some(tag_float(f, path, t, len)?).filter(|v| v.is_finite()),
            K_HIGHPASS => m.highpass = Some(tag_float(f, path, t, len)?).filter(|v| v.is_finite()),
            K_LINE_FREQ => {
                m.line_freq = Some(tag_float(f, path, t, len)?).filter(|v| v.is_finite());
            }
            K_MEAS_DATE => {
                let d = tag_data(f, path, t, len)?;
                if d.len() >= 4 {
                    let usecs = if d.len() >= 8 { be_i32(&d, 4) } else { 0 };
                    m.meas_date = Some((be_i32(&d, 0), usecs));
                }
            }
            K_UTC_OFFSET => m.utc_offset = Some(tag_text(f, path, t, len)?),
            K_EXPERIMENTER => m.experimenter = Some(tag_text(f, path, t, len)?),
            K_COMMENT => m.description = Some(tag_text(f, path, t, len)?),
            K_PROJ_NAME => m.project = Some(tag_text(f, path, t, len)?),
            _ => {}
        }
    }
    // Extended channel information (long names and the rest), one block per channel in order.
    let ext: Vec<&FifBlock> = info
        .children
        .iter()
        .filter(|b| b.kind == B_CH_INFO)
        .collect();
    if !ext.is_empty() && ext.len() == m.channels.len() {
        for (b, ch) in ext.iter().zip(m.channels.iter_mut()) {
            for t in &b.tags {
                match t.kind {
                    K_CH_SCAN_NO => ch.scan_number = tag_int(f, path, t, len)?,
                    K_CH_LOGICAL_NO => ch.logical_number = tag_int(f, path, t, len)?,
                    K_CH_KIND => ch.kind = tag_int(f, path, t, len)?,
                    K_CH_RANGE => ch.range = tag_float(f, path, t, len)? as f32,
                    K_CH_CAL => ch.cal = tag_float(f, path, t, len)? as f32,
                    K_CH_UNIT => ch.unit = tag_int(f, path, t, len)?,
                    K_CH_UNIT_MUL => ch.unit_mul = tag_int(f, path, t, len)?,
                    K_CH_COIL_TYPE => ch.coil_type = tag_int(f, path, t, len)?,
                    K_CH_DACQ_NAME => ch.name = tag_text(f, path, t, len)?,
                    _ => {}
                }
            }
        }
    }
    for b in info
        .children
        .iter()
        .filter(|b| b.kind == B_MNE_BAD_CHANNELS)
    {
        if let Some(t) = b.tag(K_MNE_CH_NAME_LIST) {
            m.bads = name_list(&tag_text(f, path, t, len)?);
        }
    }
    Ok(m)
}

/// A colon-separated name list (a colon inside a name is written as `\:`).
pub fn name_list(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&':') => {
                cur.push(':');
                chars.next();
            }
            ':' => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || !out.is_empty() {
        out.push(cur);
    }
    out
}

/// The parts that follow `path` (split files), from the reference blocks of `root`.
fn next_part(
    f: &mut SourceFile,
    path: &Path,
    root: &FifBlock,
    len: u64,
) -> Result<Option<PathBuf>> {
    for r in root.find(B_REF) {
        let mut role = None;
        let mut name = None;
        let mut num = None;
        for t in &r.tags {
            match t.kind {
                K_REF_ROLE => role = Some(tag_int(f, path, t, len)?),
                K_REF_FILE_NAME => name = Some(tag_text(f, path, t, len)?),
                K_REF_FILE_NUM => num = Some(tag_int(f, path, t, len)?),
                _ => {}
            }
        }
        if role != Some(2) {
            continue;
        }
        let dir = path.parent().unwrap_or_else(|| Path::new(""));
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            // only the file name: a reference never leaves the directory
            let leaf = Path::new(&n)
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default();
            if !leaf.as_os_str().is_empty() {
                return Ok(Some(dir.join(leaf)));
            }
        }
        if let Some(k) = num {
            let base = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(dot) = base.find('.') else {
                continue;
            };
            let (stem, ext) = (&base[..dot], &base[dot + 1..]);
            let stem = match stem.rfind('-') {
                Some(i)
                    if stem[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < stem.len() =>
                {
                    &stem[..i]
                }
                _ => stem,
            };
            return Ok(Some(dir.join(format!("{stem}-{k}.{ext}"))));
        }
    }
    Ok(None)
}

/// The raw data block of a measurement block.
fn data_block(meas: &FifBlock) -> Option<(&FifBlock, FifDataBlock)> {
    for (kind, which) in [
        (B_RAW, FifDataBlock::Raw),
        (B_CONTINUOUS, FifDataBlock::Continuous),
        (B_IAS_RAW, FifDataBlock::ActiveShieldingRaw),
    ] {
        if let Some(b) = meas.find(kind).into_iter().next() {
            return Some((b, which));
        }
    }
    None
}

/// The buffers of one part's data block, numbered on from `next_first`.
#[allow(clippy::too_many_arguments)]
fn buffers(
    f: &mut SourceFile,
    path: &Path,
    raw: &FifBlock,
    nchan: u64,
    part: usize,
    len: u64,
    first_sample: &mut Option<u64>,
    next_first: &mut u64,
    out: &mut Vec<FifBuffer>,
    skips: &mut Vec<usize>,
    findings: &mut Vec<Finding>,
) -> Result<()> {
    let mut first_skip: Option<u64> = None;
    let mut pending_skip = 0u64;
    let start = out.len();
    for t in &raw.tags {
        match t.kind {
            K_FIRST_SAMPLE if out.len() == start => {
                let v = tag_int(f, path, t, len)?;
                let v = u64::try_from(v).map_err(|_| {
                    Error::corrupt_at(FIF_FORMAT_ID, t.pos, format!("negative first sample {v}"))
                })?;
                if part == 0 {
                    *first_sample = Some(v);
                    *next_first = v;
                }
            }
            K_DATA_SKIP => {
                let v = u64::try_from(tag_int(f, path, t, len)?).unwrap_or(0);
                if out.len() == start && part == 0 {
                    first_skip = Some(v);
                } else {
                    pending_skip = pending_skip.saturating_add(v);
                }
            }
            K_DATA_BUFFER => {
                let Some(st) = FifSampleType::of(t.data_type) else {
                    let what = match t.data_type {
                        T_COMPLEX_FLOAT | T_COMPLEX_DOUBLE => {
                            "complex-valued data buffers".to_string()
                        }
                        other => format!("data buffers of type {other}"),
                    };
                    return Err(Error::unsupported(
                        FIF_FORMAT_ID,
                        what,
                        "Only 16- and 32-bit integer and 32- and 64-bit float buffers are read; MNE-Python can convert the file.",
                    ));
                };
                let row = st.bytes() * nchan;
                let samples = u64::from(t.size) / row;
                if !u64::from(t.size).is_multiple_of(row) {
                    findings.push(Finding::warning(
                        "buffer_size",
                        format!(
                            "the data buffer at byte {} holds {} bytes, not a whole number of {nchan}-channel samples",
                            t.pos, t.size
                        ),
                    ));
                }
                if t.data_pos() + u64::from(t.size) > len {
                    findings.push(Finding::error(
                        "truncated",
                        format!(
                            "the data buffer at byte {} runs past the end of the file",
                            t.pos
                        ),
                    ));
                    break;
                }
                if let Some(k) = first_skip.take() {
                    *next_first = next_first.saturating_add(k.saturating_mul(samples));
                    *first_sample = Some(*next_first);
                }
                if pending_skip > 0 {
                    *next_first = next_first.saturating_add(pending_skip.saturating_mul(samples));
                    pending_skip = 0;
                    skips.push(out.len());
                }
                if first_sample.is_none() {
                    *first_sample = Some(*next_first);
                }
                out.push(FifBuffer {
                    part,
                    data_pos: t.data_pos(),
                    sample_type: st,
                    samples,
                    first: *next_first,
                });
                *next_first = next_first.saturating_add(samples);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Annotations of the first MNE annotation block, onsets in seconds after the first sample.
fn annotations(
    f: &mut SourceFile,
    path: &Path,
    root: &FifBlock,
    len: u64,
    meas_date: Option<(i32, i32)>,
    first_time: f64,
) -> Result<Vec<FifAnnotation>> {
    let Some(b) = root.find(B_ANNOTATIONS).into_iter().next() else {
        return Ok(Vec::new());
    };
    let mut onset = Vec::new();
    let mut end = Vec::new();
    let mut texts = Vec::new();
    let mut orig: Option<f64> = None;
    for t in &b.tags {
        match t.kind {
            K_ANNOT_ONSET => onset = tag_floats(f, path, t, len)?,
            K_ANNOT_END => end = tag_floats(f, path, t, len)?,
            K_COMMENT => texts = name_list(&tag_text(f, path, t, len)?),
            K_MEAS_DATE => {
                let d = tag_data(f, path, t, len)?;
                orig = match (t.data_type, d.len()) {
                    (T_INT, 8..) => Some(f64::from(be_i32(&d, 0)) + f64::from(be_i32(&d, 4)) / 1e6),
                    (T_DOUBLE, 8..) => Some(f64::from_be_bytes([
                        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
                    ])),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    if onset.len() != end.len() || onset.len() != texts.len() {
        return Err(Error::corrupt_at(
            FIF_FORMAT_ID,
            b.tags.first().map_or(0, |t| t.pos),
            format!(
                "the annotation block has {} onsets, {} ends and {} descriptions",
                onset.len(),
                end.len(),
                texts.len()
            ),
        ));
    }
    let meas = meas_date
        .filter(|&(s, u)| !(s == 0 && u == DATE_NONE_USECS))
        .map(|(s, u)| f64::from(s) + f64::from(u) / 1e6);
    // Onsets count from the block's own origin when it has one (shifted to the measurement
    // date), from the acquisition start when neither has a date, and from the first sample
    // when only the measurement has one.
    let onset_s = |o: f64| match (orig, meas) {
        (Some(t0), Some(m)) => o + (t0 - m) - first_time,
        (Some(_) | None, None) => o - first_time,
        (None, Some(_)) => o,
    };
    Ok(onset
        .iter()
        .zip(&end)
        .zip(texts)
        .map(|((&o, &e), text)| FifAnnotation {
            onset_s: onset_s(o),
            duration_s: e - o,
            text,
        })
        .collect())
}

/// Parse a FIF file and the split-file parts it continues into.
pub fn parse_fif(fs: &Fs, f: &mut SourceFile, path: &Path, file_len: u64) -> Result<FifFile> {
    let mut findings = Vec::new();
    let (tags, from_directory) = directory(f, path, file_len, &mut findings)?;
    let id_tag = FifTag {
        kind: K_FILE_ID,
        data_type: T_ID_STRUCT,
        size: 20,
        pos: 0,
    };
    let id = tag_data(f, path, &id_tag, file_len)?;
    let version = be_i32(&id, 0);
    let file_id_secs = be_i32(&id, 12);
    let root = tree(f, path, &tags, file_len, &mut findings)?;
    let mut creators = Vec::new();
    let mut command_line = None;
    {
        let mut stack = vec![&root];
        while let Some(b) = stack.pop() {
            for t in &b.tags {
                if t.kind == K_CREATOR && creators.len() < 64 {
                    let c = tag_text(f, path, t, file_len)?;
                    if !c.trim().is_empty() && !creators.contains(&c) {
                        creators.push(c);
                    }
                } else if t.kind == K_MNE_COMMAND_LINE && command_line.is_none() {
                    command_line = Some(tag_text(f, path, t, file_len)?);
                }
            }
            stack.extend(b.children.iter().rev());
        }
    }
    let meas = root.find(B_MEAS);
    let Some(meas) = meas.first().copied() else {
        return Err(Error::unsupported(
            FIF_FORMAT_ID,
            "FIF files without a measurement block (forward solutions, source spaces, transforms, …)",
            "Only FIF files holding measurements are read; this one holds no recorded signals.",
        ));
    };
    let Some(info_block) = meas.find(B_MEAS_INFO).into_iter().next() else {
        return Err(Error::corrupt(
            FIF_FORMAT_ID,
            "the measurement block has no measurement info",
        ));
    };
    let m = meas_info(f, path, info_block, file_len)?;
    let nchan = m.nchan.unwrap_or(m.channels.len() as i32);
    if m.channels.is_empty() || usize::try_from(nchan).ok() != Some(m.channels.len()) {
        return Err(Error::corrupt(
            FIF_FORMAT_ID,
            format!(
                "the measurement info declares {nchan} channel(s) and describes {}",
                m.channels.len()
            ),
        ));
    }
    let sfreq = m
        .sfreq
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or_else(|| {
            Error::corrupt(
                FIF_FORMAT_ID,
                "the measurement info has no valid sample rate",
            )
        })?;
    let mut other_content = Vec::new();
    for (kind, what) in [
        (B_MNE_EPOCHS, "epochs"),
        (B_EVOKED, "evoked responses"),
        (B_PROCESSED, "processed data"),
    ] {
        if !meas.find(kind).is_empty() {
            other_content.push(what);
        }
    }
    let mut file = FifFile {
        file_len,
        version,
        file_id_secs,
        root: FifBlock::default(),
        from_directory,
        tag_count: tags.len(),
        sfreq,
        lowpass_hz: m.lowpass,
        highpass_hz: m.highpass,
        line_freq_hz: m.line_freq,
        meas_date: m.meas_date,
        utc_offset: m.utc_offset,
        experimenter: m.experimenter,
        description: m.description,
        project: m.project,
        bad_channels: m.bads,
        creators,
        command_line,
        data_block: None,
        other_content,
        buffers: Vec::new(),
        sweeps: Vec::new(),
        first_sample: 0,
        parts: vec![path.to_path_buf()],
        part_lens: vec![file_len],
        annotations: Vec::new(),
        findings: Vec::new(),
        channels: m.channels,
    };
    let nch = file.channels.len() as u64;
    let mut first_sample = None;
    let mut next_first = 0u64;
    let mut skips = Vec::new();
    if let Some((raw, which)) = data_block(meas) {
        file.data_block = Some(which);
        buffers(
            f,
            path,
            raw,
            nch,
            0,
            file_len,
            &mut first_sample,
            &mut next_first,
            &mut file.buffers,
            &mut skips,
            &mut findings,
        )?;
        // split files: follow the next-file references
        let mut cur_root = root.clone();
        let mut cur_path = path.to_path_buf();
        let mut handle_owned: Option<SourceFile> = None;
        while file.parts.len() < MAX_FIF_PARTS {
            let next = {
                let h = handle_owned.as_mut().unwrap_or(&mut *f);
                let len = *file.part_lens.last().expect("one part");
                next_part(h, &cur_path, &cur_root, len)?
            };
            let Some(next) = next else { break };
            if file.parts.contains(&next) {
                break;
            }
            if !fs.exists(&next) {
                findings.push(Finding::warning(
                    "split_part_missing",
                    format!(
                        "the recording continues in {}, which is not there; only the parts found are read",
                        next.display()
                    ),
                ));
                break;
            }
            let mut h = fs.open(&next).map_err(|e| Error::io(&next, e))?;
            let len = h.size().map_err(|e| Error::io(&next, e))?;
            let mut part_findings = Vec::new();
            let (ptags, _) = directory(&mut h, &next, len, &mut part_findings)?;
            let proot = tree(&mut h, &next, &ptags, len, &mut part_findings)?;
            let Some((praw, _)) = proot.find(B_MEAS).into_iter().next().and_then(data_block) else {
                break;
            };
            let part = file.parts.len();
            file.parts.push(next.clone());
            file.part_lens.push(len);
            buffers(
                &mut h,
                &next,
                praw,
                nch,
                part,
                len,
                &mut first_sample,
                &mut next_first,
                &mut file.buffers,
                &mut skips,
                &mut findings,
            )?;
            findings.extend(part_findings);
            cur_root = proot;
            cur_path = next;
            handle_owned = Some(h);
        }
    }
    file.first_sample = first_sample.unwrap_or(0);
    // sweeps: runs of buffers without a skip
    let mut start = 0usize;
    for cut in skips
        .iter()
        .copied()
        .chain(std::iter::once(file.buffers.len()))
    {
        if cut > start {
            let b0 = &file.buffers[start];
            let samples = file.buffers[start..cut].iter().map(|b| b.samples).sum();
            file.sweeps.push(FifSweep {
                first_buffer: start,
                end_buffer: cut,
                first: b0.first,
                samples,
            });
        }
        start = cut;
    }
    let first_time = file.first_sample as f64 / file.sfreq;
    file.annotations = annotations(f, path, &root, file_len, file.meas_date, first_time)?;
    file.root = root;
    file.findings = findings;
    Ok(file)
}

impl FifFile {
    /// `FIFF major.minor` from the file id.
    pub fn version_text(&self) -> String {
        format!("{}.{}", self.version >> 16, self.version & 0xffff)
    }

    /// The measurement date as ISO 8601 UTC with microseconds.
    pub fn recorded_at(&self) -> Option<String> {
        let (s, u) = self.meas_date?;
        if (s == 0 && u == DATE_NONE_USECS) || !(0..1_000_000).contains(&u) {
            return None;
        }
        let secs = i64::from(s);
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (y, mo, d) = openreadout_core::time::civil_from_days(days);
        Some(format!(
            "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{u:06}Z",
            sod / 3600,
            (sod % 3600) / 60,
            sod % 60
        ))
    }

    /// Bytes of one sample of every channel in buffer `b`.
    pub fn row_bytes(&self, b: &FifBuffer) -> u64 {
        b.sample_type.bytes() * self.channels.len() as u64
    }
}

/// The FIF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct FifReader;

impl FormatReader for FifReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::FIF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FIF_FORMAT_ID.into(),
            name: "FIF (MEGIN / Elekta Neuromag MEG and EEG, MNE)".into(),
            vendor: "MEGIN (Elekta Neuromag); also written by MNE-Python and MNE-C".into(),
            extensions: vec!["fif".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::FIF.confidence,
            known_gaps: vec![
                "Raw and continuous data only: epochs, evoked responses, forward and inverse solutions are refused".into(),
                "Projections and gradient compensation stored in the file are listed, not applied".into(),
                "Compressed `.fif.gz` files are not read; decompress them first".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_fif(head) {
            return Some(Detection {
                format_id: FIF_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["fif"]).then(|| Detection {
            format_id: FIF_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("FIF extension but no FIF file-id tag".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FifDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FifDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened FIF file.
#[derive(Debug)]
pub struct FifDataset {
    path: PathBuf,
    fs: Fs,
    handles: Vec<Option<SourceFile>>,
    file: FifFile,
    texts: Vec<String>,
}

impl FifDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let mut f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let file_len = f.size().map_err(|e| Error::io(&path, e))?;
        let file = parse_fif(&fs, &mut f, &path, file_len)?;
        if file.data_block.is_none() {
            let what = if file.other_content.is_empty() {
                "FIF measurements without raw data".to_string()
            } else {
                format!("FIF files of {}", file.other_content.join(" and "))
            };
            return Err(Error::unsupported(
                FIF_FORMAT_ID,
                what,
                "Only raw and continuous recordings are read; MNE-Python reads epochs and evoked files.",
            ));
        }
        let mut texts: Vec<String> = Vec::new();
        for a in &file.annotations {
            if !texts.contains(&a.text) {
                texts.push(a.text.clone());
            }
        }
        let mut handles: Vec<Option<SourceFile>> = (0..file.parts.len()).map(|_| None).collect();
        handles[0] = Some(f);
        Ok(FifDataset {
            path,
            fs,
            handles,
            file,
            texts,
        })
    }

    /// The parsed file.
    pub fn fif(&self) -> &FifFile {
        &self.file
    }

    fn handle(&mut self, part: usize) -> Result<&mut SourceFile> {
        if self.handles[part].is_none() {
            let p = &self.file.parts[part];
            self.handles[part] = Some(self.fs.open(p).map_err(|e| Error::io(p, e))?);
        }
        Ok(self.handles[part].as_mut().expect("just opened"))
    }

    fn trace_info(&self) -> TraceInfo {
        let f = &self.file;
        let dtype = f.buffers.first().map_or("int16", |b| b.sample_type.dtype());
        let channels = f
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut e = BTreeMap::new();
                if let Some(k) = kind_name(c.kind) {
                    e.insert("kind".into(), json!(k));
                } else {
                    e.insert("kind_code".into(), json!(c.kind));
                }
                e.insert("coil_type".into(), json!(c.coil_type));
                e.insert("logical_number".into(), json!(c.logical_number));
                e.insert("scan_number".into(), json!(c.scan_number));
                e.insert("range".into(), json!(f64::from(c.range)));
                e.insert("cal".into(), json!(f64::from(c.cal)));
                if c.unit_mul != 0 {
                    e.insert("unit_mul".into(), json!(c.unit_mul));
                }
                if f.bad_channels.contains(&c.name) {
                    e.insert("bad".into(), json!(true));
                }
                if c.loc.iter().any(|v| *v != 0.0 && v.is_finite()) {
                    e.insert(
                        "position_m".into(),
                        json!([
                            f64::from(c.loc[0]),
                            f64::from(c.loc[1]),
                            f64::from(c.loc[2])
                        ]),
                    );
                }
                SignalChannelInfo {
                    index: i as u32,
                    name: if c.name.is_empty() {
                        format!("ch{i}")
                    } else {
                        c.name.clone()
                    },
                    unit: unit_name(c.unit).map(str::to_string),
                    dtype: dtype.into(),
                    scale: c.factor(),
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("first_sample".into(), json!(f.first_sample));
        extra.insert("data_buffers".into(), json!(f.buffers.len()));
        if let Some(b) = f.data_block {
            extra.insert("data_block".into(), json!(b.name()));
        }
        if let Some(t) = f.recorded_at() {
            extra.insert("recorded_at".into(), json!(t));
        }
        for (k, v) in [
            ("lowpass_hz", f.lowpass_hz),
            ("highpass_hz", f.highpass_hz),
            ("line_freq_hz", f.line_freq_hz),
        ] {
            if let Some(v) = v {
                extra.insert(k.into(), json!(v));
            }
        }
        for (k, v) in [
            ("experimenter", &f.experimenter),
            ("description", &f.description),
            ("project", &f.project),
        ] {
            if let Some(v) = v.as_ref().filter(|v| !v.trim().is_empty()) {
                extra.insert(k.into(), json!(v.trim()));
            }
        }
        if !f.bad_channels.is_empty() {
            extra.insert("bad_channels".into(), json!(f.bad_channels));
        }
        if !f.creators.is_empty() {
            extra.insert("creators".into(), json!(f.creators));
        }
        if f.parts.len() > 1 {
            extra.insert("split_parts".into(), json!(f.parts.len()));
        }
        let counts: Vec<u64> = f.sweeps.iter().map(|s| s.samples).collect();
        if f.sweeps.len() > 1 {
            let t0 = f.first_sample as f64;
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    f.sweeps
                        .iter()
                        .map(|s| (s.first as f64 - t0) / f.sfreq)
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        TraceInfo {
            index: 0,
            name: Some(f.data_block.map_or("raw", FifDataBlock::name).into()),
            sample_rate_hz: f.sfreq,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: u32::try_from(f.sweeps.len()).unwrap_or(u32::MAX),
            channels,
            start_s: Some(f.first_sample as f64 / f.sfreq),
            extra,
        }
    }

    fn annotation_table(&self) -> Option<TableInfo> {
        if self.file.annotations.is_empty() {
            return None;
        }
        let col = |i: u32, name: &str, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: i,
            name: name.into(),
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let mut text = col(2, "text", "int32", None);
        text.extra.insert("categories".into(), json!(self.texts));
        Some(TableInfo {
            index: 0,
            name: Some("annotations".into()),
            row_count: self.file.annotations.len() as u64,
            columns: vec![
                col(0, "onset_s", "float64", Some("s")),
                col(1, "duration_s", "float64", Some("s")),
                text,
            ],
            extra: BTreeMap::new(),
        })
    }
}

/// Decode big-endian samples of one buffer row range into channels, × each channel's factor.
fn decode(bytes: &[u8], st: FifSampleType, factors: &[f64], out: &mut [Vec<f64>]) {
    let n = factors.len();
    match st {
        FifSampleType::Int16 => {
            for (k, x) in bytes.as_chunks::<2>().0.iter().enumerate() {
                let c = k % n;
                out[c].push(f64::from(i16::from_be_bytes(*x)) * factors[c]);
            }
        }
        FifSampleType::Int32 => {
            for (k, x) in bytes.as_chunks::<4>().0.iter().enumerate() {
                let c = k % n;
                out[c].push(f64::from(i32::from_be_bytes(*x)) * factors[c]);
            }
        }
        FifSampleType::Float32 => {
            for (k, x) in bytes.as_chunks::<4>().0.iter().enumerate() {
                let c = k % n;
                out[c].push(f64::from(f32::from_be_bytes(*x)) * factors[c]);
            }
        }
        FifSampleType::Float64 => {
            for (k, x) in bytes.as_chunks::<8>().0.iter().enumerate() {
                let c = k % n;
                out[c].push(f64::from_be_bytes(*x) * factors[c]);
            }
        }
    }
}

fn block_name(kind: i32) -> String {
    match kind {
        B_ROOT => "root".into(),
        B_MEAS => "measurement".into(),
        B_MEAS_INFO => "measurement info".into(),
        B_RAW => "raw data".into(),
        B_PROCESSED => "processed data".into(),
        B_EVOKED => "evoked".into(),
        105 => "aspect".into(),
        106 => "subject".into(),
        107 => "digitizer points".into(),
        108 => "HPI measurement".into(),
        109 => "HPI result".into(),
        110 => "HPI coil".into(),
        111 => "project".into(),
        B_CONTINUOUS => "continuous data".into(),
        B_CH_INFO => "channel info".into(),
        115 => "events".into(),
        117 => "acquisition parameters".into(),
        B_REF => "file reference".into(),
        B_IAS_RAW => "active shielding raw data".into(),
        313 => "projections".into(),
        314 => "projection item".into(),
        B_MNE_BAD_CHANNELS => "bad channels".into(),
        B_MNE_EPOCHS => "epochs".into(),
        B_PROCESSING_HISTORY => "processing history".into(),
        901 => "processing record".into(),
        B_ANNOTATIONS => "annotations".into(),
        k => format!("block {k}"),
    }
}

impl Dataset for FifDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let samples: u64 = f.sweeps.iter().map(|s| s.samples).sum();
        let mut notes = vec![format!(
            "FIF {} file: {} channel(s) at {} Hz, {} {} data buffer(s), {samples} samples per channel",
            f.version_text(),
            f.channels.len(),
            f.sfreq,
            f.buffers.len(),
            f.data_block.map_or("raw", FifDataBlock::name),
        )];
        notes.push(
            "values are stored numbers × range × cal per channel, in SI units (T, T/m, V); projections and compensation are not applied"
                .into(),
        );
        if f.data_block == Some(FifDataBlock::ActiveShieldingRaw) {
            notes.push(
                "recorded with internal active shielding (MaxShield): process with SSS (MaxFilter, MNE maxwell_filter) before analysis"
                    .into(),
            );
        }
        if f.sweeps.len() > 1 {
            notes.push(format!(
                "{} skip(s) in the data: the {} runs of contiguous buffers are sweeps (`extra.sweep_starts_s`)",
                f.sweeps.len() - 1,
                f.sweeps.len()
            ));
        }
        if f.parts.len() > 1 {
            notes.push(format!(
                "split recording: {} part files are read as one (`member_files`)",
                f.parts.len()
            ));
        }
        if !f.annotations.is_empty() {
            notes.push(format!(
                "{} annotation(s) in the `annotations` table (onset and duration in seconds after the first sample)",
                f.annotations.len()
            ));
        }
        if !f.other_content.is_empty() {
            notes.push(format!(
                "the file also holds {}, which are not read",
                f.other_content.join(" and ")
            ));
        }
        if f.findings
            .iter()
            .any(|x| x.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.part_lens.iter().sum(),
            format: FifReader.descriptor(),
            format_version: Some(f.version_text()),
            images: Vec::new(),
            tables: self.annotation_table().into_iter().collect(),
            spectra: Vec::new(),
            traces: vec![self.trace_info()],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "file_id": {"version": f.version, "secs": f.file_id_secs},
            "tag_count": f.tag_count,
            "from_directory": f.from_directory,
            "meas_date": f.meas_date.map(|(s, u)| json!([s, u])),
            "utc_offset": f.utc_offset,
            "sfreq": f.sfreq,
            "lowpass": f.lowpass_hz,
            "highpass": f.highpass_hz,
            "line_freq": f.line_freq_hz,
            "experimenter": f.experimenter,
            "description": f.description,
            "project": f.project,
            "bad_channels": f.bad_channels,
            "creators": f.creators,
            "command_line": f.command_line,
            "channels": f.channels.iter().map(|c| json!({
                "name": c.name, "kind": c.kind, "coil_type": c.coil_type,
                "logical_number": c.logical_number, "scan_number": c.scan_number,
                "range": f64::from(c.range), "cal": f64::from(c.cal),
                "unit": c.unit, "unit_mul": c.unit_mul,
                "loc": c.loc.iter().map(|v| f64::from(*v)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "first_sample": f.first_sample,
            "parts": f.parts.iter().map(|p| p.file_name().map(|s| s.to_string_lossy().into_owned())).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "traces[].sample_rate_hz",
            "traces[].sample_count",
            "traces[].sweep_count",
            "traces[].start_s",
            "traces[].channels[].name",
            "traces[].channels[].unit",
            "traces[].channels[].scale",
            "traces[].channels[].offset",
            "traces[].extra.recorded_at",
            "traces[].extra.sweep_starts_s",
            "traces[].extra.first_sample",
            "tables[].row_count",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let f = &self.file;
        let mut out = Vec::new();
        let mut stack = vec![(&f.root, 0usize)];
        while let Some((b, depth)) = stack.pop() {
            out.push(LsEntry {
                kind: "block".into(),
                name: format!("{}{}", "  ".repeat(depth), block_name(b.kind)),
                offset: b.tags.first().map(|t| t.pos),
                size: None,
                image: None,
                details: json!({"block_kind": b.kind, "tags": b.tags.len(), "children": b.children.len()}),
            });
            for c in b.children.iter().rev() {
                stack.push((c, depth + 1));
            }
        }
        let bytes: u64 = f.buffers.iter().map(|b| b.samples * f.row_bytes(b)).sum();
        out.push(LsEntry {
            kind: "data-buffers".into(),
            name: format!("{} data buffers", f.buffers.len()),
            offset: f.buffers.first().map(|b| b.data_pos),
            size: Some(bytes),
            image: None,
            details: json!({
                "sweeps": f.sweeps.len(),
                "sample_type": f.buffers.first().map(|b| b.sample_type.dtype()),
                "parts": f.parts.len(),
            }),
        });
        Ok(out)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        self.file.parts.clone()
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FIF_FORMAT_ID,
            "image planes",
            "FIF recordings hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace index {index} out of range (1 trace)"
            )));
        }
        let Some(sw) = self.file.sweeps.get(sweep as usize).cloned() else {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} sweeps)",
                self.file.sweeps.len()
            )));
        };
        if first > sw.samples {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({} samples)",
                sw.samples
            )));
        }
        let nch = self.file.channels.len();
        let factors: Vec<f64> = self.file.channels.iter().map(FifChannel::factor).collect();
        let count = max.min(sw.samples - first).min(MAX_FIF_READ);
        let mut channels: Vec<Vec<f64>> = vec![Vec::new(); nch];
        let mut budget = MAX_FIF_READ_BYTES;
        let mut at = sw.first + first; // absolute sample number
        let mut got = 0u64;
        for bi in sw.first_buffer..sw.end_buffer {
            if got >= count {
                break;
            }
            let b = self.file.buffers[bi];
            if b.first + b.samples <= at {
                continue;
            }
            let row = self.file.row_bytes(&b).max(1);
            let lo = at - b.first;
            // the byte budget, but always at least one sample
            let rows = (budget / row).max(u64::from(got == 0));
            let take = (b.samples - lo).min(count - got).min(rows);
            if take == 0 {
                break;
            }
            let len = take * row;
            let pos = b.data_pos + lo * row;
            let part_len = self.file.part_lens[b.part];
            let path = self.file.parts[b.part].clone();
            let block = read_block(self.handle(b.part)?, &path, pos, len, part_len)?;
            if (block.bytes.len() as u64) < len {
                return Err(Error::corrupt_at(
                    FIF_FORMAT_ID,
                    pos,
                    "a data buffer is cut off by the end of the file",
                ));
            }
            decode(&block.bytes, b.sample_type, &factors, &mut channels);
            budget = budget.saturating_sub(len);
            got += take;
            at += take;
            if lo + take < b.samples {
                break; // the budget ended inside this buffer
            }
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 || self.file.annotations.is_empty() {
            return Err(Error::Usage(format!("table index {index} out of range")));
        }
        let rows = &self.file.annotations;
        let a = usize::try_from(first_row)
            .unwrap_or(usize::MAX)
            .min(rows.len());
        let b = a
            .saturating_add(usize::try_from(max_rows).unwrap_or(usize::MAX))
            .min(rows.len());
        let slice = &rows[a..b];
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: vec![
                slice.iter().map(|r| r.onset_s).collect(),
                slice.iter().map(|r| r.duration_s).collect(),
                slice
                    .iter()
                    .map(|r| {
                        self.texts
                            .iter()
                            .position(|t| t == &r.text)
                            .map_or(f64::NAN, |i| i as f64)
                    })
                    .collect(),
            ],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), FIF_FORMAT_ID);
        r.performed("tag directory read (or every tag header followed) and nested into blocks");
        r.performed(
            "measurement info parsed: channel count against the channel infos, sample rate",
        );
        r.performed("every data buffer's type, size and extent checked against the file");
        for x in &f.findings {
            r.push(x.clone());
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &f.channels {
            if !seen.insert(c.name.as_str()) {
                r.push(Finding::warning(
                    "duplicate_channel_name",
                    format!("channel name {:?} appears more than once", c.name),
                ));
            }
            if c.range == 0.0 || c.cal == 0.0 {
                r.push(Finding::warning(
                    "zero_calibration",
                    format!(
                        "channel {:?} has range {} and cal {}: its values read as 0",
                        c.name, c.range, c.cal
                    ),
                ));
            }
        }
        if f.buffers
            .windows(2)
            .any(|w| w[0].sample_type != w[1].sample_type)
        {
            r.push(Finding::info(
                "mixed_buffer_types",
                "the data buffers do not all have the same sample type",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
