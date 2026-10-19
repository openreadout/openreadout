//! Igor packed experiments (`.pxp`, `.pxt`): a chain of records, each an 8-byte header (record
//! kind, version, length of the data) and its data. Wave records hold one binary wave
//! ([`crate::wave`]); data-folder start and end records nest the waves and variables that follow
//! them; variables records hold numeric and string variables; text records hold the history,
//! the recreation procedures and the procedure window. Every other kind is skipped. Layout and
//! vocabulary: `docs/formats/igor.md`.

use openreadout_core::bytes::Endian;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::wave::{Problem, igor_text};

/// Bytes of a record header.
pub const RECORD_HEADER: u64 = 8;
/// Records walked at most.
pub const MAX_RECORDS: usize = 4_000_000;
/// Bytes of text kept per text record (history, procedures).
pub const MAX_RECORD_TEXT: u64 = 1 << 20;
/// Bytes of a variables record decoded at most.
pub const MAX_VARIABLES_BYTES: u64 = 16 << 20;
/// Bit of the record kind that marks a superseded record.
pub const SUPERSEDED: u16 = 0x8000;

/// Record kinds this reader reads (PTN003; the numbers from the corpus files).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// Variables of the current data folder.
    Variables,
    /// History text.
    History,
    /// One binary wave.
    Wave,
    /// Recreation procedures (text).
    Recreation,
    /// The procedure window's text.
    Procedures,
    /// Marks where the history is applied (no data).
    GetHistory,
    /// A packed notebook or procedure file.
    PackedFile,
    /// Start of a data folder (32-byte name).
    FolderStart,
    /// End of the current data folder.
    FolderEnd,
    /// Any other kind (window, package and platform settings): skipped.
    Other(u16),
}

impl RecordKind {
    /// The kind of record-header value `v` (superseded bit removed).
    pub fn from_code(v: u16) -> RecordKind {
        match v & !SUPERSEDED {
            1 => RecordKind::Variables,
            2 => RecordKind::History,
            3 => RecordKind::Wave,
            4 => RecordKind::Recreation,
            5 => RecordKind::Procedures,
            7 => RecordKind::GetHistory,
            8 => RecordKind::PackedFile,
            9 => RecordKind::FolderStart,
            10 => RecordKind::FolderEnd,
            o => RecordKind::Other(o),
        }
    }

    /// Our name of the kind.
    pub fn name(self) -> String {
        match self {
            RecordKind::Variables => "variables".into(),
            RecordKind::History => "history".into(),
            RecordKind::Wave => "wave".into(),
            RecordKind::Recreation => "recreation procedures".into(),
            RecordKind::Procedures => "procedures".into(),
            RecordKind::GetHistory => "get history".into(),
            RecordKind::PackedFile => "packed file".into(),
            RecordKind::FolderStart => "data folder start".into(),
            RecordKind::FolderEnd => "data folder end".into(),
            RecordKind::Other(v) => format!("kind {v}"),
        }
    }
}

/// One record.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Its kind.
    pub kind: RecordKind,
    /// The raw kind field (with the superseded bit).
    pub code: u16,
    /// The record-header version.
    pub version: u16,
    /// File offset of the record header.
    pub offset: u64,
    /// Bytes of data after the header.
    pub len: u64,
    /// The data folder the record belongs to (`root`, `root:sub`, …).
    pub folder: String,
}

impl Record {
    /// File offset of the record's data.
    pub fn data_offset(&self) -> u64 {
        self.offset + RECORD_HEADER
    }
    /// True when a later record replaces this one.
    pub fn superseded(&self) -> bool {
        self.code & SUPERSEDED != 0
    }
}

/// The record chain of a packed experiment.
#[derive(Debug, Clone)]
pub struct Packed {
    /// Byte order of every header and value.
    pub endian: Endian,
    /// Records in file order.
    pub records: Vec<Record>,
    /// Problems found while walking.
    pub problems: Vec<Problem>,
}

/// Byte order of a packed experiment from its first record header, when it looks like one: a
/// known kind (or a small one), version 0 and a length inside `file_len`.
pub fn packed_endian(head: &[u8], file_len: u64) -> Option<Endian> {
    // follow the chain inside the head: the order under which more records chain wins
    let walk = |e: Endian| -> usize {
        let mut pos = 0u64;
        let mut n = 0usize;
        while let Some(at) = usize::try_from(pos).ok().filter(|p| p + 8 <= head.len()) {
            let Some(len) = fits_at(head, at, e, file_len, pos) else {
                break;
            };
            n += 1;
            pos += RECORD_HEADER + len;
        }
        n
    };
    let le = walk(Endian::Little);
    let be = walk(Endian::Big);
    match (le, be) {
        (0, 0) => None,
        (l, b) if b > l => Some(Endian::Big),
        _ => Some(Endian::Little),
    }
}

fn fits_at(head: &[u8], at: usize, e: Endian, file_len: u64, pos: u64) -> Option<u64> {
    let kind = e.u16(head, at)? & !SUPERSEDED;
    let version = e.u16(head, at + 2)?;
    let len = u64::try_from(e.i32(head, at + 4)?).ok()?;
    ((1..=127).contains(&kind) && version < 16 && pos + RECORD_HEADER + len <= file_len)
        .then_some(len)
}

/// True when `head` (the first bytes of a file of `file_len` bytes) starts a record chain that
/// holds a wave record or a data-folder record in its first records.
pub fn looks_like_packed(head: &[u8], file_len: u64) -> bool {
    let Some(e) = packed_endian(head, file_len) else {
        return false;
    };
    let mut pos = 0usize;
    let mut kinds = Vec::new();
    while pos + 8 <= head.len() && kinds.len() < 64 {
        let Some(len) = fits_at(head, pos, e, file_len, pos as u64) else {
            break;
        };
        kinds.push(RecordKind::from_code(e.u16(head, pos).unwrap_or(0)));
        let Some(next) = usize::try_from(len)
            .ok()
            .and_then(|l| (pos + 8).checked_add(l))
        else {
            break;
        };
        pos = next;
    }
    // the chain must reach past the head or the end of the file, and hold Igor's text or data
    // records (a short chain of small kinds is too weak alone)
    let complete = pos >= head.len() || pos as u64 == file_len;
    complete
        && kinds.len() >= 2
        && kinds.iter().any(|k| {
            matches!(
                k,
                RecordKind::Wave
                    | RecordKind::Variables
                    | RecordKind::FolderStart
                    | RecordKind::History
                    | RecordKind::Recreation
                    | RecordKind::Procedures
            )
        })
}

/// Walk every record header of the file.
pub fn walk(f: &SourceFile, file_len: u64) -> Result<Packed> {
    let mut head = vec![0u8; usize::try_from(file_len.min(64 * 1024)).unwrap_or(0)];
    f.read_exact_at(0, &mut head).map_err(|e| {
        Error::corrupt_at(FORMAT_ID, 0, format!("cannot read the first record: {e}"))
    })?;
    let endian = packed_endian(&head, file_len).ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            "not an Igor packed experiment: the first 8 bytes are not a record header",
        )
    })?;
    let mut records = Vec::new();
    let mut problems = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    let mut pos = 0u64;
    let mut hdr = [0u8; 8];
    while pos < file_len {
        if records.len() >= MAX_RECORDS {
            problems.push(Problem {
                code: "too_many_records",
                detail: format!("more than {MAX_RECORDS} records; the rest are not read"),
                offset: Some(pos),
                fatal: false,
            });
            break;
        }
        if pos + RECORD_HEADER > file_len {
            problems.push(Problem {
                code: "truncated",
                detail: format!(
                    "{} bytes at the end are too few for a record header",
                    file_len - pos
                ),
                offset: Some(pos),
                fatal: true,
            });
            break;
        }
        f.read_exact_at(pos, &mut hdr)
            .map_err(|e| Error::corrupt_at(FORMAT_ID, pos, format!("cannot read a record: {e}")))?;
        let code = endian.u16(&hdr, 0).unwrap_or(0);
        let version = endian.u16(&hdr, 2).unwrap_or(0);
        let len = endian.i32(&hdr, 4).unwrap_or(-1);
        let Ok(len) = u64::try_from(len) else {
            problems.push(Problem {
                code: "bad_record",
                detail: format!("record at {pos} declares a negative length {len}"),
                offset: Some(pos),
                fatal: true,
            });
            break;
        };
        if pos + RECORD_HEADER + len > file_len {
            problems.push(Problem {
                code: "truncated",
                detail: format!("record at {pos} ({len} bytes) runs past the end of the file"),
                offset: Some(pos),
                fatal: true,
            });
            break;
        }
        let kind = RecordKind::from_code(code);
        let folder = std::iter::once("root".to_string())
            .chain(folders.iter().cloned())
            .collect::<Vec<_>>()
            .join(":");
        let superseded = code & SUPERSEDED != 0;
        if !superseded {
            match kind {
                RecordKind::FolderStart => {
                    let mut b = vec![0u8; usize::try_from(len.min(32)).unwrap_or(0)];
                    f.read_exact_at(pos + RECORD_HEADER, &mut b)
                        .map_err(|e| Error::corrupt_at(FORMAT_ID, pos, e.to_string()))?;
                    let name = igor_text(openreadout_core::bytes::until_nul(&b));
                    folders.push(name.trim().to_string());
                }
                RecordKind::FolderEnd if folders.pop().is_none() => {
                    problems.push(Problem {
                        code: "folders",
                        detail: format!("data folder end at {pos} without a start"),
                        offset: Some(pos),
                        fatal: false,
                    });
                }
                _ => {}
            }
        }
        records.push(Record {
            kind,
            code,
            version,
            offset: pos,
            len,
            folder,
        });
        pos += RECORD_HEADER + len;
    }
    if !folders.is_empty() {
        problems.push(Problem {
            code: "folders",
            detail: format!("{} data folder(s) are not closed", folders.len()),
            offset: None,
            fatal: false,
        });
    }
    Ok(Packed {
        endian,
        records,
        problems,
    })
}

/// One variable of a variables record.
#[derive(Debug, Clone, PartialEq)]
pub enum Variable {
    /// A numeric variable: real and imaginary part (0 for real variables).
    Number {
        /// Name.
        name: String,
        /// Real part.
        real: f64,
        /// Imaginary part.
        imaginary: f64,
        /// The numeric-type field (4 = double, 5 = complex double).
        number_type: i16,
    },
    /// A string variable.
    Text {
        /// Name.
        name: String,
        /// Value.
        value: String,
    },
}

/// The decoded variables of one record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variables {
    /// The data folder.
    pub folder: String,
    /// Variables-header version (1 or 2).
    pub version: i16,
    /// System variables K0, K1, … (read in the root folder only).
    pub system: Vec<f32>,
    /// User variables in file order.
    pub user: Vec<Variable>,
    /// What could not be decoded, when anything.
    pub undecoded: Option<String>,
}

/// Bytes of a numeric variable.
pub const NUMBER_VARIABLE: usize = 56;
/// Bytes of a variable name field.
pub const NAME_FIELD: usize = 32;

/// Decode a variables record's data.
pub fn parse_variables(b: &[u8], e: Endian, folder: &str) -> Variables {
    let mut v = Variables {
        folder: folder.to_string(),
        ..Variables::default()
    };
    let i16_ = |o: usize| e.i16(b, o);
    let Some(version) = i16_(0) else {
        v.undecoded = Some("the record is empty".into());
        return v;
    };
    v.version = version;
    let (n_sys, n_num, n_str, n_dep_num, n_dep_str, mut pos) = match version {
        1 => (i16_(2), i16_(4), i16_(6), Some(0), Some(0), 8usize),
        2 => (i16_(2), i16_(4), i16_(6), i16_(8), i16_(10), 12usize),
        _ => {
            v.undecoded = Some(format!("variables header version {version}"));
            return v;
        }
    };
    let (Some(n_sys), Some(n_num), Some(n_str), Some(n_dep_num), Some(n_dep_str)) =
        (n_sys, n_num, n_str, n_dep_num, n_dep_str)
    else {
        v.undecoded = Some("the variables header is cut short".into());
        return v;
    };
    let count = |n: i16| usize::try_from(n).ok();
    let (Some(n_sys), Some(n_num), Some(n_str)) = (count(n_sys), count(n_num), count(n_str)) else {
        v.undecoded = Some("negative variable counts".into());
        return v;
    };
    for k in 0..n_sys {
        let Some(x) = e.f32(b, pos + 4 * k) else {
            v.undecoded = Some("system variables cut short".into());
            return v;
        };
        if folder == "root" {
            v.system.push(x);
        }
    }
    pos += 4 * n_sys;
    let name_at = |p: usize| -> Option<String> {
        let f = b.get(p..p + NAME_FIELD)?;
        Some(
            igor_text(openreadout_core::bytes::until_nul(f))
                .trim()
                .to_string(),
        )
    };
    for _ in 0..n_num {
        let (Some(name), Some(nt), Some(re), Some(im)) = (
            name_at(pos),
            e.i16(b, pos + 34),
            e.f64(b, pos + 36),
            e.f64(b, pos + 44),
        ) else {
            v.undecoded = Some("numeric variables cut short".into());
            return v;
        };
        v.user.push(Variable::Number {
            name,
            real: re,
            imaginary: im,
            number_type: nt,
        });
        pos += NUMBER_VARIABLE;
    }
    for _ in 0..n_str {
        let Some(name) = name_at(pos) else {
            v.undecoded = Some("string variables cut short".into());
            return v;
        };
        let (len, skip) = if version == 1 {
            (e.i16(b, pos + NAME_FIELD).map(i64::from), 2)
        } else {
            (e.i32(b, pos + NAME_FIELD).map(i64::from), 4)
        };
        let Some(len) = len.and_then(|l| usize::try_from(l).ok()) else {
            v.undecoded = Some("a string variable has no valid length".into());
            return v;
        };
        let start = pos + NAME_FIELD + skip;
        let Some(text) = start.checked_add(len).and_then(|end| b.get(start..end)) else {
            v.undecoded = Some("string variables cut short".into());
            return v;
        };
        v.user.push(Variable::Text {
            name,
            value: igor_text(text),
        });
        pos = start + len;
    }
    if n_dep_num > 0 || n_dep_str > 0 {
        v.undecoded = Some(format!(
            "{n_dep_num} dependent numeric and {n_dep_str} dependent string variables are not decoded"
        ));
    }
    v
}
