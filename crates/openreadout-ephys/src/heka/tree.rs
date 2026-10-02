//! HEKA's "Tree" container and the pulsed tree (`.pul`) inside a bundle: Root → Group → Series →
//! Sweep → Trace records, each followed by its child count. Record sizes are read from the tree
//! itself (they differ between PatchMaster versions); a field past a record's stored size is
//! absent. Offsets from HEKA's published `PulsedFile_v9.txt` (`docs/formats/heka-patchmaster.md`).

use openreadout_core::bytes::{latin1_field, le_f64, le_i16, le_i32, le_u16};
use openreadout_core::{Error, Result};

use super::HEKA_FORMAT_ID;

/// Levels of the pulsed tree (root, group, series, sweep, trace).
pub const PULSED_LEVELS: usize = 5;
/// Records a tree may hold at most (bounds memory on a damaged file).
pub const MAX_TREE_RECORDS: usize = 4_000_000;
/// Largest record size accepted, bytes.
pub const MAX_RECORD_LEN: u32 = 1 << 20;

fn text(r: &[u8], at: usize, n: usize) -> String {
    at.checked_add(n)
        .and_then(|end| r.get(at..end))
        .map(latin1_field)
        .unwrap_or_default()
}

/// The recording mode of a trace (`RecordingModeType`).
pub fn recording_mode_name(code: u8) -> &'static str {
    match code {
        0 => "inside-out",
        1 => "on-cell",
        2 => "outside-out",
        3 => "whole-cell",
        4 => "current-clamp",
        5 => "voltage-clamp",
        6 => "no-mode",
        _ => "unknown",
    }
}

/// Stored sample type of a trace (`DataFormatType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    /// 16-bit signed integers.
    Int16,
    /// 32-bit signed integers.
    Int32,
    /// 32-bit IEEE floats.
    Float32,
    /// 64-bit IEEE floats.
    Float64,
}

impl SampleType {
    /// From `TrDataFormat`.
    pub fn from_code(c: u8) -> Option<Self> {
        match c {
            0 => Some(Self::Int16),
            1 => Some(Self::Int32),
            2 => Some(Self::Float32),
            3 => Some(Self::Float64),
            _ => None,
        }
    }
    /// Bytes per sample.
    pub fn width(self) -> u64 {
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
    /// True for the integer types (scaled with the data scaler).
    pub fn is_integer(self) -> bool {
        matches!(self, Self::Int16 | Self::Int32)
    }
}

/// One trace record (a channel of one sweep).
#[derive(Debug, Clone)]
pub struct TraceRecord {
    /// `TrLabel`.
    pub label: String,
    /// Byte offset of the first sample in the file (`TrData`).
    pub data_offset: u64,
    /// Samples (`TrDataPoints`).
    pub points: u64,
    /// `TrDataKind` bits: 0 little-endian, 1 leak, 2 virtual, 3 Imon, 4 Vmon, 5 clipped.
    pub data_kind: u16,
    /// `TrRecordingMode`.
    pub recording_mode: u8,
    /// `TrDataFormat` code.
    pub format_code: u8,
    /// Scale from stored integers to the unit (`TrDataScaler`).
    pub scaler: f64,
    /// Offset of this channel's first sample within the sweep, seconds (`TrTimeOffset`).
    pub time_offset_s: f64,
    /// The zero level PatchMaster can subtract (`TrZeroData`), in the unit.
    pub zero_offset: f64,
    /// Unit of the values (`A`, `V`, ...).
    pub unit: String,
    /// Sample interval (`TrXInterval`).
    pub x_interval: f64,
    /// Time of the first sample (`TrXStart`).
    pub x_start: f64,
    /// Unit of the x axis (`s`).
    pub x_unit: String,
    /// `TrYOffset`.
    pub y_offset: f64,
    /// `TrBandwidth`, Hz.
    pub bandwidth_hz: Option<f64>,
    /// `TrLinkDAChannel`.
    pub linked_dac: Option<i32>,
    /// `TrAdcChannel`.
    pub adc_channel: Option<i16>,
    /// `TrInterleaveSize` (0 = contiguous).
    pub interleave_size: u64,
    /// `TrInterleaveSkip`.
    pub interleave_skip: u64,
    /// Series resistance, pipette and seal resistance, membrane capacitance when recorded.
    pub rs_ohm: Option<f64>,
    /// `TrCSlow` (farads).
    pub c_slow_f: Option<f64>,
    /// `TrSealResistance` (ohms).
    pub seal_ohm: Option<f64>,
    /// `TrPipetteResistance` (ohms).
    pub pipette_ohm: Option<f64>,
    /// `TrTrHolding` (the holding level, in the stimulus unit), when the record has it.
    pub holding: Option<f64>,
}

impl TraceRecord {
    /// The sample type, if the code is known.
    pub fn sample_type(&self) -> Option<SampleType> {
        SampleType::from_code(self.format_code)
    }
    /// Bit `n` of the data kind.
    pub fn kind_bit(&self, n: u16) -> bool {
        self.data_kind & (1 << n) != 0
    }
}

/// One sweep.
#[derive(Debug, Clone)]
pub struct SweepRecord {
    /// `SwLabel`.
    pub label: String,
    /// `SwTime`, PatchMaster seconds.
    pub time: f64,
    /// `SwTimer`, seconds since the timer was reset.
    pub timer: f64,
    /// `SwStimCount`: the stimulation (PGF) record it used, 1-based.
    pub stim_count: i32,
    /// `SwSweepCount`, 1-based.
    pub sweep_count: i32,
    /// `SwTemperature`, when recorded.
    pub temperature: Option<f64>,
    /// Its trace records.
    pub traces: Vec<TraceRecord>,
}

/// One series.
#[derive(Debug, Clone)]
pub struct SeriesRecord {
    /// `SeLabel`.
    pub label: String,
    /// `SeComment`.
    pub comment: String,
    /// `SeSeriesCount`, 1-based.
    pub series_count: i32,
    /// `SeTime`, PatchMaster seconds.
    pub time: f64,
    /// `SeMethodName`, when present.
    pub method: String,
    /// `SeUsername`, when present.
    pub username: String,
    /// Its sweeps.
    pub sweeps: Vec<SweepRecord>,
}

/// One group (usually one cell or experiment).
#[derive(Debug, Clone)]
pub struct GroupRecord {
    /// `GrLabel`.
    pub label: String,
    /// `GrText`.
    pub text: String,
    /// `GrExperimentNumber`.
    pub experiment_number: i32,
    /// Its series.
    pub series: Vec<SeriesRecord>,
}

/// The pulsed tree.
#[derive(Debug, Clone)]
pub struct PulsedTree {
    /// `RoVersion` (9 for PatchMaster 2x60–2x90.2, 1000 for 2x90.3 and later).
    pub root_version: i32,
    /// `RoVersionName`.
    pub version_name: String,
    /// `RoRootText`.
    pub root_text: String,
    /// `RoStartTime`, PatchMaster seconds.
    pub start_time: f64,
    /// The stored size of each level's records.
    pub record_sizes: Vec<u32>,
    /// Groups.
    pub groups: Vec<GroupRecord>,
    /// Byte length of the tree as walked.
    pub walked_len: u64,
}

struct Walker<'a> {
    b: &'a [u8],
    pos: usize,
    sizes: Vec<u32>,
    records: usize,
}

impl<'a> Walker<'a> {
    fn record(&mut self, level: usize) -> Result<(&'a [u8], usize)> {
        self.records += 1;
        if self.records > MAX_TREE_RECORDS {
            return Err(Error::unsupported(
                HEKA_FORMAT_ID,
                format!("pulsed trees with more than {MAX_TREE_RECORDS} records"),
                "the file's pulsed tree is larger than this reader accepts; it may be damaged.",
            ));
        }
        let size = self.sizes[level] as usize;
        let at = self.pos;
        let rec = self
            .b
            .get(at..at + size)
            .ok_or_else(|| self.cut(level, "record"))?;
        let n = le_i32(self.b, at + size).ok_or_else(|| self.cut(level, "child count"))?;
        self.pos = at + size + 4;
        if n < 0 {
            return Err(Error::corrupt(
                HEKA_FORMAT_ID,
                format!("pulsed tree: a level-{level} record has a negative child count"),
            ));
        }
        let n = n as usize;
        // every child needs at least its child count: bound the claimed count by the bytes left
        if n > 0 && (level + 1 >= self.sizes.len() || n > (self.b.len() - self.pos) / 4) {
            return Err(Error::corrupt(
                HEKA_FORMAT_ID,
                format!(
                    "pulsed tree: a level-{level} record claims {n} children that the tree cannot hold"
                ),
            ));
        }
        Ok((rec, n))
    }
    fn cut(&self, level: usize, what: &str) -> Error {
        Error::corrupt(
            HEKA_FORMAT_ID,
            format!(
                "pulsed tree: a level-{level} {what} at tree byte {} runs past the end of the tree ({} bytes)",
                self.pos,
                self.b.len()
            ),
        )
    }
}

fn opt(r: &[u8], at: usize) -> Option<f64> {
    le_f64(r, at).filter(|v| v.is_finite())
}

fn trace(r: &[u8]) -> TraceRecord {
    TraceRecord {
        label: text(r, 4, 32),
        data_offset: le_i32(r, 40).map_or(0, |v| v.max(0) as u64),
        points: le_i32(r, 44).map_or(0, |v| v.max(0) as u64),
        data_kind: le_u16(r, 64).unwrap_or(0),
        recording_mode: r.get(68).copied().unwrap_or(6),
        format_code: r.get(70).copied().unwrap_or(0),
        scaler: le_f64(r, 72).unwrap_or(f64::NAN),
        time_offset_s: le_f64(r, 80).unwrap_or(0.0),
        zero_offset: le_f64(r, 88).unwrap_or(0.0),
        unit: text(r, 96, 8),
        x_interval: le_f64(r, 104).unwrap_or(f64::NAN),
        x_start: le_f64(r, 112).unwrap_or(0.0),
        x_unit: text(r, 120, 8),
        y_offset: le_f64(r, 136).unwrap_or(0.0),
        bandwidth_hz: opt(r, 144),
        pipette_ohm: opt(r, 152),
        seal_ohm: opt(r, 168),
        c_slow_f: opt(r, 176),
        rs_ohm: opt(r, 192),
        linked_dac: le_i32(r, 216),
        adc_channel: le_i16(r, 222),
        interleave_size: le_i32(r, 292).map_or(0, |v| v.max(0) as u64),
        interleave_skip: le_i32(r, 296).map_or(0, |v| v.max(0) as u64),
        holding: opt(r, 408),
    }
}

/// Parse the pulsed tree held in `b` (the whole `.pul` item).
pub fn parse_pulsed(b: &[u8]) -> Result<PulsedTree> {
    let magic = b
        .get(..4)
        .ok_or_else(|| Error::corrupt(HEKA_FORMAT_ID, "the pulsed tree is empty"))?;
    match magic {
        b"eerT" => {}
        b"Tree" => {
            return Err(Error::unsupported(
                HEKA_FORMAT_ID,
                "big-endian pulsed trees",
                "the pulsed tree was written big-endian (a PowerPC Mac); open the file in PatchMaster on a current computer and save it again.",
            ));
        }
        _ => {
            return Err(Error::corrupt(
                HEKA_FORMAT_ID,
                "the .pul item does not start with the Tree magic number",
            ));
        }
    }
    let levels = le_i32(b, 4).unwrap_or(0);
    if levels != PULSED_LEVELS as i32 {
        return Err(Error::unsupported(
            HEKA_FORMAT_ID,
            format!("pulsed trees with {levels} levels"),
            "PatchMaster pulsed trees have five levels (root, group, series, sweep, trace); this file's tree is different and is not read.",
        ));
    }
    let mut sizes = Vec::with_capacity(PULSED_LEVELS);
    for l in 0..PULSED_LEVELS {
        let at = 8 + 4 * l;
        let s = le_i32(b, at)
            .ok_or_else(|| Error::corrupt(HEKA_FORMAT_ID, "pulsed tree header cut off"))?;
        if s <= 0 || s as u32 > MAX_RECORD_LEN {
            return Err(Error::corrupt(
                HEKA_FORMAT_ID,
                format!("pulsed tree: level {l} record size {s} is not usable"),
            ));
        }
        sizes.push(s as u32);
    }
    // the record fields this reader needs must lie inside the stored records
    for (level, need, what) in [
        (0usize, 528usize, "root"),
        (1, 120, "group"),
        (2, 144, "series"),
        (3, 64, "sweep"),
        (4, 128, "trace"),
    ] {
        if (sizes[level] as usize) < need {
            return Err(Error::unsupported(
                HEKA_FORMAT_ID,
                format!(
                    "{what} records of {} bytes (this reader needs at least {need})",
                    sizes[level]
                ),
                "the file comes from a PatchMaster or PULSE version whose records are shorter than those this reader was validated on.",
            ));
        }
    }
    let mut walker = Walker {
        b,
        pos: 8 + 4 * PULSED_LEVELS,
        sizes: sizes.clone(),
        records: 0,
    };
    let (root, ngroups) = walker.record(0)?;
    let mut tree = PulsedTree {
        root_version: le_i32(root, 0).unwrap_or(0),
        version_name: text(root, 8, 32),
        root_text: text(root, 120, 400),
        start_time: le_f64(root, 520).unwrap_or(f64::NAN),
        record_sizes: sizes,
        groups: Vec::new(),
        walked_len: 0,
    };
    for _ in 0..ngroups {
        let (group_rec, nseries) = walker.record(1)?;
        let mut group = GroupRecord {
            label: text(group_rec, 4, 32),
            text: text(group_rec, 36, 80),
            experiment_number: le_i32(group_rec, 116).unwrap_or(0),
            series: Vec::new(),
        };
        for _ in 0..nseries {
            let (series_rec, nsweeps) = walker.record(2)?;
            let mut series = SeriesRecord {
                label: text(series_rec, 4, 32),
                comment: text(series_rec, 36, 80),
                series_count: le_i32(series_rec, 116).unwrap_or(0),
                time: le_f64(series_rec, 136).unwrap_or(f64::NAN),
                method: text(series_rec, 312, 32),
                username: text(series_rec, 872, 80),
                sweeps: Vec::new(),
            };
            for _ in 0..nsweeps {
                let (sweep_rec, ntraces) = walker.record(3)?;
                let mut sweep = SweepRecord {
                    label: text(sweep_rec, 4, 32),
                    time: le_f64(sweep_rec, 48).unwrap_or(f64::NAN),
                    timer: le_f64(sweep_rec, 56).unwrap_or(f64::NAN),
                    stim_count: le_i32(sweep_rec, 40).unwrap_or(0),
                    sweep_count: le_i32(sweep_rec, 44).unwrap_or(0),
                    temperature: opt(sweep_rec, 96),
                    traces: Vec::new(),
                };
                for _ in 0..ntraces {
                    let (trace_rec, children) = walker.record(4)?;
                    if children != 0 {
                        return Err(Error::corrupt(
                            HEKA_FORMAT_ID,
                            "pulsed tree: a trace record has children",
                        ));
                    }
                    sweep.traces.push(trace(trace_rec));
                }
                series.sweeps.push(sweep);
            }
            group.series.push(series);
        }
        tree.groups.push(group);
    }
    tree.walked_len = walker.pos as u64;
    Ok(tree)
}

/// Seconds between PatchMaster's stored time and the Unix epoch, following HEKA's
/// `TimeFormat.txt` (subtract `JanFirst1990`, wrap negatives by 2^32, then shift to 1601 and on
/// to 1970).
pub fn heka_time_to_unix(t: f64) -> Option<f64> {
    const JAN_FIRST_1990: f64 = 1_580_970_496.0;
    const HIGH_DWORD: f64 = 4_294_967_296.0;
    const MAC_BASE: f64 = 9_561_652_096.0; // to 1601-01-01
    const FILETIME_TO_UNIX: f64 = 11_644_473_600.0;
    if !t.is_finite() || t == 0.0 {
        return None;
    }
    let mut x = t - JAN_FIRST_1990;
    if x < 0.0 {
        x += HIGH_DWORD;
    }
    Some(x + MAC_BASE - FILETIME_TO_UNIX)
}

/// A stored PatchMaster time as ISO-8601 without a zone (PatchMaster records the acquisition
/// computer's wall clock), to the millisecond.
pub fn heka_time_iso(t: f64) -> Option<String> {
    let u = heka_time_to_unix(t)?;
    let secs = u.floor();
    let millis = ((u - secs) * 1000.0).floor().clamp(0.0, 999.0) as u32;
    let s = openreadout_core::time::unix_to_iso8601(secs as i64, millis);
    Some(s.trim_end_matches('Z').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_time_examples() {
        // TimeFormat.txt example 2: 4922414972 -> 19-Nov-2009
        let s = heka_time_iso(4_922_414_972.0).unwrap();
        assert_eq!(s.get(..10), Some("2009-11-19"));
        // example 1: 221667551 -> 09-Jan-1997
        let s = heka_time_iso(221_667_551.0).unwrap();
        assert_eq!(s.get(..10), Some("1997-01-09"));
        assert!(!s.ends_with('Z'));
    }

    fn tree_bytes(sizes: [i32; 5], counts: &[(usize, i32)]) -> Vec<u8> {
        let mut b = b"eerT".to_vec();
        b.extend(5i32.to_le_bytes());
        for s in sizes {
            b.extend(s.to_le_bytes());
        }
        for &(level, n) in counts {
            b.extend(vec![0u8; sizes[level] as usize]);
            b.extend(n.to_le_bytes());
        }
        b
    }

    #[test]
    fn walks_a_minimal_tree() {
        let sizes = [640, 144, 1408, 288, 512];
        let b = tree_bytes(
            sizes,
            &[(0, 1), (1, 1), (2, 2), (3, 1), (4, 0), (3, 1), (4, 0)],
        );
        let t = parse_pulsed(&b).unwrap();
        assert_eq!(t.groups.len(), 1);
        assert_eq!(t.groups[0].series[0].sweeps.len(), 2);
        assert_eq!(t.walked_len, b.len() as u64);
    }

    #[test]
    fn rejects_impossible_child_counts_and_cut_trees() {
        let sizes = [640, 144, 1408, 288, 512];
        let b = tree_bytes(sizes, &[(0, 1_000_000)]);
        assert!(parse_pulsed(&b).is_err());
        let b = tree_bytes(sizes, &[(0, 1), (1, 1)]);
        assert!(parse_pulsed(&b).is_err());
        let mut b = tree_bytes(sizes, &[(0, 0)]);
        b[..4].copy_from_slice(b"Tree");
        assert!(parse_pulsed(&b).is_err());
    }

    #[test]
    fn short_records_are_refused() {
        let b = tree_bytes([640, 144, 1408, 288, 100], &[(0, 0)]);
        assert!(parse_pulsed(&b).is_err());
    }
}
