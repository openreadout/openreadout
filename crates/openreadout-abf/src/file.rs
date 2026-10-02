//! Parse an ABF 1 or ABF 2 header into one normalized description (`AbfFile`).
//!
//! Byte offsets are listed in `docs/formats/abf.md`; their sources in `docs/provenance/abf.md`.

use std::path::Path;

use openreadout_core::model::Finding;
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use openreadout_core::bytes::{Block, latin1_field, read_block};

/// Bytes per header block (section and data pointers count in these).
pub const BLOCK_LEN: u64 = 512;
/// ABF 1 header length when the extended fields are present (versions ≥ 1.6).
pub const ABF1_EXTENDED_LEN: u64 = 6144;
/// ABF 1 header length of old files (data starts at block 4).
pub const ABF1_BASIC_LEN: u64 = 2048;
/// Bytes of an ABF 2 section-map entry (u32 block, u32 entry size, i64 entry count).
pub const SECTION_ENTRY_LEN: u64 = 16;
/// Offset of the ABF 2 section map.
pub const SECTION_MAP_OFFSET: u64 = 76;
/// Most synch-array or tag entries read (guards allocation on a damaged header).
pub const MAX_RECORDS: u64 = 1_000_000;
/// Longest string block read from an ABF 2 strings section.
pub const MAX_STRINGS_LEN: u64 = 1 << 20;

/// ABF 2 section-map entries in file order, in our names.
pub const SECTION_NAMES: [&str; 18] = [
    "protocol",
    "adc",
    "dac",
    "epoch",
    "adc_per_dac",
    "epoch_per_dac",
    "user_list",
    "stats_region",
    "math",
    "strings",
    "data",
    "tag",
    "scope",
    "delta",
    "voice_tag",
    "synch_array",
    "annotation",
    "stats",
];

/// Which of the two unrelated header layouts the file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// Signature `ABF ` (fixed 2048- or 6144-byte header).
    Abf1,
    /// Signature `ABF2` (512-byte header plus a section map).
    Abf2,
}

impl Generation {
    pub fn name(self) -> &'static str {
        match self {
            Generation::Abf1 => "abf1",
            Generation::Abf2 => "abf2",
        }
    }
}

/// How the recording was acquired (header operation-mode code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionMode {
    /// 1: triggered sweeps of varying length.
    EventVariableLength,
    /// 2: triggered sweeps of fixed length.
    EventFixedLength,
    /// 3: one continuous recording.
    GapFree,
    /// 4: high-speed oscilloscope.
    HighSpeedOscilloscope,
    /// 5: episodic stimulation (sweeps with a command waveform).
    Episodic,
    Other(i16),
}

impl AcquisitionMode {
    pub fn from_code(c: i16) -> Self {
        match c {
            1 => AcquisitionMode::EventVariableLength,
            2 => AcquisitionMode::EventFixedLength,
            3 => AcquisitionMode::GapFree,
            4 => AcquisitionMode::HighSpeedOscilloscope,
            5 => AcquisitionMode::Episodic,
            o => AcquisitionMode::Other(o),
        }
    }
    pub fn code(self) -> i16 {
        match self {
            AcquisitionMode::EventVariableLength => 1,
            AcquisitionMode::EventFixedLength => 2,
            AcquisitionMode::GapFree => 3,
            AcquisitionMode::HighSpeedOscilloscope => 4,
            AcquisitionMode::Episodic => 5,
            AcquisitionMode::Other(c) => c,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            AcquisitionMode::EventVariableLength => "event-variable-length",
            AcquisitionMode::EventFixedLength => "event-fixed-length",
            AcquisitionMode::GapFree => "gap-free",
            AcquisitionMode::HighSpeedOscilloscope => "high-speed-oscilloscope",
            AcquisitionMode::Episodic => "episodic",
            AcquisitionMode::Other(_) => "unknown",
        }
    }
}

/// Stored sample type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Signed 16-bit ADC counts, scaled per channel.
    Int16,
    /// IEEE float32 already in physical units.
    Float32,
}

impl SampleFormat {
    pub fn width(self) -> u64 {
        match self {
            SampleFormat::Int16 => 2,
            SampleFormat::Float32 => 4,
        }
    }
    pub fn dtype(self) -> &'static str {
        match self {
            SampleFormat::Int16 => "int16",
            SampleFormat::Float32 => "float32",
        }
    }
}

/// One entry of the ABF 2 section map (or a fixed ABF 1 region), in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionEntry {
    pub name: &'static str,
    pub block: u32,
    pub entry_size: u32,
    pub entry_count: u64,
    pub offset: u64,
    /// Bytes the section occupies (for `strings`: one block of `entry_size` bytes).
    pub byte_len: u64,
}

impl SectionEntry {
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.byte_len)
    }
}

/// Amplifier telegraph values recorded for one input channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Telegraph {
    pub enabled: bool,
    pub instrument_code: i16,
    pub additional_gain: f32,
    pub filter_hz: f32,
    pub membrane_capacitance: f32,
    pub clamp_mode_code: i16,
}

/// One recorded input (ADC) channel and how its counts become physical units.
#[derive(Debug, Clone, PartialEq)]
pub struct InputChannel {
    /// Position in the interleaved data (0-based).
    pub index: u32,
    /// Physical ADC number on the digitizer.
    pub adc_number: i16,
    pub name: String,
    pub unit: String,
    pub programmable_gain: f32,
    /// Volts at the ADC per user unit.
    pub instrument_scale: f32,
    pub instrument_offset: f32,
    pub signal_gain: f32,
    pub signal_offset: f32,
    pub lowpass_hz: Option<f32>,
    pub highpass_hz: Option<f32>,
    pub telegraph: Option<Telegraph>,
    /// `value = raw × scale + offset` (1 and 0 for float32 data).
    pub scale: f64,
    pub offset: f64,
}

/// Kind of one epoch of a command waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochKind {
    Off,
    Step,
    Ramp,
    PulseTrain,
    TriangleTrain,
    CosineTrain,
    BiphasicTrain,
    Other(i16),
}

impl EpochKind {
    pub fn from_code(c: i16) -> Self {
        match c {
            0 => EpochKind::Off,
            1 => EpochKind::Step,
            2 => EpochKind::Ramp,
            3 => EpochKind::PulseTrain,
            4 => EpochKind::TriangleTrain,
            5 => EpochKind::CosineTrain,
            7 => EpochKind::BiphasicTrain,
            o => EpochKind::Other(o),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            EpochKind::Off => "off",
            EpochKind::Step => "step",
            EpochKind::Ramp => "ramp",
            EpochKind::PulseTrain => "pulse-train",
            EpochKind::TriangleTrain => "triangle-train",
            EpochKind::CosineTrain => "cosine-train",
            EpochKind::BiphasicTrain => "biphasic-train",
            EpochKind::Other(_) => "unknown",
        }
    }
}

/// One row of a command-waveform (epoch) table. Levels are in the output channel's unit,
/// durations and pulse timings in samples; `*_step` is added per sweep.
#[derive(Debug, Clone, PartialEq)]
pub struct Epoch {
    pub index: u32,
    pub kind: EpochKind,
    pub kind_code: i16,
    pub level: f32,
    pub level_step: f32,
    pub duration: i32,
    pub duration_step: i32,
    pub pulse_period: Option<i32>,
    pub pulse_width: Option<i32>,
}

/// One analog output (DAC) channel: holding level and command waveform.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputChannel {
    pub index: u32,
    pub name: String,
    pub unit: String,
    pub holding_level: f32,
    pub waveform_enabled: bool,
    pub waveform_source_code: i16,
    pub stimulus_file: Option<String>,
    pub epochs: Vec<Epoch>,
    /// Between sweeps the output stays at the last epoch level instead of the holding level.
    pub inter_episode_last: bool,
    /// A conditioning (pre-sweep) train is enabled on this output.
    pub conditioning: bool,
}

/// A digital-output pattern for one epoch (bit n = output n).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigitalEpoch {
    pub epoch: u32,
    pub pattern: u16,
}

/// A comment placed during acquisition.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub raw_time: i32,
    pub time_s: f64,
    pub comment: String,
    pub kind_code: i16,
}

/// Where one sweep's samples are in the interleaved data.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    /// First sample (per channel) of the sweep within the data.
    pub first_sample: u64,
    /// Samples per channel.
    pub sample_count: u64,
    /// Start relative to the recording start, seconds, when the file records it.
    pub start_s: Option<f64>,
}

/// Everything read from the header of an ABF file.
#[derive(Debug, Clone)]
pub struct AbfFile {
    pub generation: Generation,
    /// `2.6.0.0` (ABF 2) or `1.83` (ABF 1, a float in the header).
    pub version: String,
    pub file_len: u64,
    /// Bytes of header actually interpreted (ABF 1: 2048 or 6144; ABF 2: the fixed 512).
    pub header_len: u64,
    pub mode: AcquisitionMode,
    pub sample_format: SampleFormat,
    pub data_offset: u64,
    /// Samples of all channels together.
    pub total_samples: u64,
    /// Per channel, microseconds.
    pub sample_interval_us: f64,
    pub sample_rate_hz: f64,
    /// Sweep count as written in the header (before the gap-free and 0 → 1 rules).
    pub header_sweep_count: i64,
    pub sweeps: Vec<Sweep>,
    pub channels: Vec<InputChannel>,
    pub outputs: Vec<OutputChannel>,
    pub digital: Vec<DigitalEpoch>,
    pub adc_range_v: f32,
    pub adc_resolution: i32,
    pub synch_time_unit_us: f32,
    pub sweep_interval_s: Option<f64>,
    pub creator: Option<String>,
    pub creator_version: Option<String>,
    pub protocol_path: Option<String>,
    pub comment: Option<String>,
    pub created_at: Option<String>,
    pub guid: Option<String>,
    pub experiment_kind_code: Option<i16>,
    pub digitizer_code: Option<i16>,
    pub tags: Vec<Tag>,
    /// Raw synch-array records `(start, length)`.
    pub synch: Vec<(i32, i32)>,
    pub sections: Vec<SectionEntry>,
    /// ABF 2 string table (index 0 = empty).
    pub strings: Vec<String>,
    /// ABF 2: a user list entry varies a protocol parameter from sweep to sweep.
    pub user_list_active: bool,
    /// ABF 2: DAC outputs alternate between sweeps.
    pub alternate_dac_output: bool,
    pub findings: Vec<Finding>,
}

impl AbfFile {
    /// Samples per channel in the data, total across sweeps.
    pub fn samples_per_channel(&self) -> u64 {
        self.total_samples / self.channels.len().max(1) as u64
    }

    /// Byte length of the data region implied by the header.
    pub fn data_len(&self) -> u64 {
        self.total_samples
            .saturating_mul(self.sample_format.width())
    }

    /// The protocol name: file stem of the protocol path when it names a `.pro` file (an
    /// unsaved protocol is recorded as `(untitled)`).
    pub fn protocol_name(&self) -> Option<String> {
        let p = self.protocol_path.as_deref()?;
        let leaf = p.rsplit(['\\', '/']).next().unwrap_or(p);
        let (stem, ext) = leaf.rsplit_once('.')?;
        (ext.eq_ignore_ascii_case("pro") && !stem.is_empty()).then(|| stem.to_string())
    }

    /// Longest sweep, samples per channel.
    pub fn max_sweep_len(&self) -> u64 {
        self.sweeps
            .iter()
            .map(|s| s.sample_count)
            .max()
            .unwrap_or(0)
    }

    pub fn variable_length(&self) -> bool {
        let first = self.sweeps.first().map(|s| s.sample_count);
        self.sweeps.iter().any(|s| Some(s.sample_count) != first)
    }

    pub fn open(path: &Path) -> Result<AbfFile> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<AbfFile> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let head = read_block(&mut f, path, 0, ABF1_EXTENDED_LEN, file_len)?;
        match head.bytes.get(..4) {
            Some(b"ABF ") => parse_abf1(&mut f, path, head, file_len),
            Some(b"ABF2") => parse_abf2(&mut f, path, &head, file_len),
            _ => Err(Error::corrupt_at(
                FORMAT_ID,
                0,
                "no `ABF ` or `ABF2` signature at byte 0",
            )),
        }
    }
}

/// Per-channel scale and offset: `1 / instrument_scale / signal_gain / programmable_gain
/// [/ telegraph gain] × adc_range / adc_resolution`, offset `instrument_offset − signal_offset`,
/// evaluated in this order in f64 (the order pyABF documents).
pub fn channel_scaling(
    instrument_scale: f32,
    signal_gain: f32,
    programmable_gain: f32,
    telegraph_gain: Option<f32>,
    adc_range: f32,
    adc_resolution: i32,
    instrument_offset: f32,
    signal_offset: f32,
) -> (f64, f64) {
    let mut g = 1.0f64;
    g /= f64::from(instrument_scale);
    g /= f64::from(signal_gain);
    g /= f64::from(programmable_gain);
    if let Some(t) = telegraph_gain {
        g /= f64::from(t);
    }
    g *= f64::from(adc_range);
    g /= f64::from(adc_resolution);
    let mut o = 0.0f64;
    o += f64::from(instrument_offset);
    o -= f64::from(signal_offset);
    (g, o)
}

/// `YYYYMMDD` or `YYMMDD` (80–99 → 19xx) plus milliseconds since midnight, as ISO-8601.
pub fn iso_datetime(date: i64, ms_of_day: i64) -> Option<String> {
    if date <= 0 || ms_of_day < 0 {
        return None;
    }
    let (y, m, d) = if date >= 1_000_000 {
        (date / 10_000, (date / 100) % 100, date % 100)
    } else {
        let yy = date / 10_000;
        let y = if yy >= 80 { 1900 + yy } else { 2000 + yy };
        (y, (date / 100) % 100, date % 100)
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=2200).contains(&y) {
        return None;
    }
    if ms_of_day >= 86_400_000 {
        return None;
    }
    let s = ms_of_day / 1000;
    let ms = ms_of_day % 1000;
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}",
        s / 3600,
        (s / 60) % 60,
        s % 60
    ))
}

/// A GUID in the usual text form (first three fields little-endian); `None` when all zero.
pub fn guid_text(b: &[u8]) -> Option<String> {
    if b.len() != 16 || b.iter().all(|&x| x == 0) {
        return None;
    }
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut s = String::new();
    for (i, &k) in order.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{:02X}", b[k]));
    }
    Some(s)
}

/// Split an ABF 2 string block into the string table (index 0 is the empty string).
pub fn parse_string_table(block: &[u8]) -> Vec<String> {
    let mut out = vec![String::new()];
    let (body, declared) = if block.len() >= 20 && &block[..4] == b"SSCH" {
        let n = u32::from_le_bytes([block[8], block[9], block[10], block[11]]);
        let mut start = 20;
        while start < block.len() && block[start] == 0 {
            start += 1;
        }
        (&block[start..], Some(n as usize))
    } else {
        (block, None)
    };
    for part in body.split(|&c| c == 0) {
        if declared.is_some_and(|n| out.len() > n) {
            break;
        }
        out.push(latin1_field(part));
    }
    if let Some(n) = declared {
        out.truncate(n + 1);
    }
    while out.len() > 1 && out.last().is_some_and(String::is_empty) && declared.is_none() {
        out.pop();
    }
    out
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

/// Sweep extents: from the synch array when it lists one entry per sweep with differing lengths
/// (variable-length acquisition), otherwise equal division of the data.
fn layout_sweeps(
    total_samples: u64,
    channels: u64,
    sweep_count: u64,
    (synch, synch_unit_us): (&[(i32, i32)], f32),
    rate_hz: f64,
    sweep_interval_s: Option<f64>,
    findings: &mut Vec<Finding>,
) -> Vec<Sweep> {
    let channels = channels.max(1);
    let per_channel = total_samples / channels;
    if !total_samples.is_multiple_of(channels) {
        findings.push(Finding::warning(
            "partial_sample_group",
            format!(
                "{total_samples} samples is not a multiple of {channels} channels; the last {} are ignored",
                total_samples % channels
            ),
        ));
    }
    let start_of = |raw: i32| -> Option<f64> {
        if synch_unit_us > 0.0 && synch_unit_us.is_finite() {
            Some(f64::from(raw) * f64::from(synch_unit_us) / 1e6)
        } else if rate_hz > 0.0 {
            Some(f64::from(raw) / (rate_hz * channels as f64))
        } else {
            None
        }
    };
    let synch_matches = synch.len() as u64 == sweep_count && sweep_count > 0;
    let lengths_differ = synch_matches && synch.iter().any(|s| s.1 != synch[0].1);
    if lengths_differ {
        let mut out = Vec::with_capacity(synch.len());
        let mut first = 0u64;
        for &(start, len) in synch {
            let n = u64::try_from(len.max(0)).unwrap_or(0) / channels;
            out.push(Sweep {
                first_sample: first,
                sample_count: n,
                start_s: start_of(start),
            });
            first = first.saturating_add(n);
        }
        if first != per_channel {
            findings.push(Finding::warning(
                "synch_length_mismatch",
                format!(
                    "synch array lengths add up to {first} samples per channel but the data holds {per_channel}"
                ),
            ));
        }
        return out;
    }
    let mut sweep_count = sweep_count.max(1);
    // The sweep count is a header field: more sweeps than samples per channel can only be
    // empty ones, and a damaged count (2^31) would otherwise allocate tens of GB of records.
    if sweep_count > per_channel.max(1) {
        findings.push(Finding::warning(
            "sweep_count_exceeds_samples",
            format!(
                "the header declares {sweep_count} sweeps but the data holds {per_channel} samples per channel; {} sweeps are listed",
                per_channel.max(1)
            ),
        ));
        sweep_count = per_channel.max(1);
    }
    let len = per_channel / sweep_count;
    if !per_channel.is_multiple_of(sweep_count) {
        findings.push(Finding::warning(
            "sweep_length_mismatch",
            format!(
                "{per_channel} samples per channel do not divide into {sweep_count} equal sweeps; the last {} are ignored",
                per_channel % sweep_count
            ),
        ));
    }
    let len_s = if rate_hz > 0.0 {
        len as f64 / rate_hz
    } else {
        0.0
    };
    (0..sweep_count)
        .map(|i| Sweep {
            first_sample: i * len,
            sample_count: len,
            start_s: if synch_matches {
                start_of(synch[i as usize].0)
            } else {
                Some(i as f64 * sweep_interval_s.filter(|v| *v > 0.0).unwrap_or(len_s))
            },
        })
        .collect()
}

fn read_records(
    f: &mut SourceFile,
    path: &Path,
    offset: u64,
    count: u64,
    size: u64,
    file_len: u64,
) -> Result<Block> {
    let count = count.min(MAX_RECORDS);
    read_block(f, path, offset, count.saturating_mul(size), file_len)
}

fn read_synch(block: &Block, count: u64, entry: u64) -> Vec<(i32, i32)> {
    (0..count)
        .map_while(|i| {
            let at = block.origin + i * entry;
            Some((block.i32_at(at)?, block.i32_at(at + 4)?))
        })
        .collect()
}

fn read_tags(block: &Block, count: u64, entry: u64, to_seconds: impl Fn(i32) -> f64) -> Vec<Tag> {
    (0..count)
        .map_while(|i| {
            let at = block.origin + i * entry;
            let raw_time = block.i32_at(at)?;
            Some(Tag {
                raw_time,
                time_s: to_seconds(raw_time),
                comment: block.text_at(at + 4, 56)?,
                kind_code: block.i16_at(at + 60)?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- ABF 1

#[allow(clippy::many_single_char_names)]
fn parse_abf1(f: &mut SourceFile, path: &Path, head: Block, file_len: u64) -> Result<AbfFile> {
    let need = |v: Option<i32>, what: &str, at: u64| {
        v.ok_or_else(|| {
            Error::corrupt_at(
                FORMAT_ID,
                at,
                format!("header ends before {what} (file is {file_len} bytes)"),
            )
        })
    };
    let h = &head;
    let mut findings = Vec::new();
    let version_f = h
        .f32_at(4)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "header shorter than 8 bytes"))?;
    let mode_code = h.i16_at(8).unwrap_or(0);
    let acq_len = need(h.i32_at(10), "the sample count", 10)?;
    let ignored = h.i16_at(14).unwrap_or(0);
    let episodes = need(h.i32_at(16), "the sweep count", 16)?;
    let date = h.i32_at(20).unwrap_or(0);
    let time_s = h.i32_at(24).unwrap_or(0);
    let data_block = need(h.i32_at(40), "the data pointer", 40)?;
    let tag_block = h.i32_at(44).unwrap_or(0);
    let tag_count = h.i32_at(48).unwrap_or(0);
    let synch_block = h.i32_at(92).unwrap_or(0);
    let synch_count = h.i32_at(96).unwrap_or(0);
    let data_format = h.i16_at(100).unwrap_or(0);
    let nch = h
        .i16_at(120)
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, 120, "header ends before the channel count"))?;
    let interval = h.f32_at(122).unwrap_or(0.0);
    let synch_unit = h.f32_at(130).unwrap_or(0.0);
    let episode_interval = h.f32_at(178).unwrap_or(0.0);
    let adc_range = h.f32_at(244).unwrap_or(0.0);
    let adc_resolution = h.i32_at(252).unwrap_or(0);
    if !(1..=16).contains(&nch) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            120,
            format!("channel count {nch} is outside 1..=16"),
        ));
    }
    if data_block <= 0 || acq_len < 0 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            40,
            format!("data pointer {data_block} / sample count {acq_len} are invalid"),
        ));
    }
    let sample_format = match data_format {
        0 => SampleFormat::Int16,
        1 => SampleFormat::Float32,
        other => {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                100,
                format!("data format code {other} is neither 0 (int16) nor 1 (float32)"),
            ));
        }
    };
    let data_offset = (data_block as u64) * BLOCK_LEN
        + u64::try_from(ignored.max(0)).unwrap_or(0) * sample_format.width();
    // Extended fields exist only when the header region reaches 6144 bytes.
    let header_len = if data_offset >= ABF1_EXTENDED_LEN {
        ABF1_EXTENDED_LEN
    } else {
        ABF1_BASIC_LEN.min(data_offset)
    };
    let extended = header_len >= ABF1_EXTENDED_LEN && head.end() >= ABF1_EXTENDED_LEN;
    if head.end() < header_len {
        findings.push(
            Finding::error(
                "truncated",
                format!(
                    "the file ends at byte {} inside the {header_len}-byte header",
                    head.end()
                ),
            )
            .at(head.end()),
        );
    }
    let nch_u = nch as usize;
    let seq: Vec<i16> = (0..nch_u)
        .map(|i| h.i16_at(410 + 2 * i as u64).unwrap_or(i as i16))
        .collect();
    let arr_f32 = |base: u64, i: usize| h.f32_at(base + 4 * i as u64);
    let arr_i16 = |base: u64, i: usize| h.i16_at(base + 2 * i as u64);
    let mut channels = Vec::with_capacity(nch_u);
    for (i, &phys) in seq.iter().enumerate() {
        let p = if (0..16).contains(&phys) {
            phys as usize
        } else {
            findings.push(Finding::warning(
                "bad_channel_map",
                format!("sampling sequence entry {i} is {phys}, outside 0..=15; using {i}"),
            ));
            i
        };
        let name = h.text_at(442 + 10 * p as u64, 10).unwrap_or_default();
        let unit = h.text_at(602 + 8 * p as u64, 8).unwrap_or_default();
        let programmable_gain = arr_f32(730, p).unwrap_or(1.0);
        let instrument_scale = arr_f32(922, p).unwrap_or(1.0);
        let instrument_offset = arr_f32(986, p).unwrap_or(0.0);
        let signal_gain = arr_f32(1050, p).unwrap_or(1.0);
        let signal_offset = arr_f32(1114, p).unwrap_or(0.0);
        let lowpass = arr_f32(1178, p);
        let highpass = arr_f32(1242, p);
        let telegraph = if extended {
            Some(Telegraph {
                enabled: arr_i16(4512, p) == Some(1),
                instrument_code: arr_i16(4544, p).unwrap_or(0),
                additional_gain: arr_f32(4576, p).unwrap_or(1.0),
                filter_hz: arr_f32(4640, p).unwrap_or(0.0),
                membrane_capacitance: arr_f32(4704, p).unwrap_or(0.0),
                clamp_mode_code: arr_i16(4768, p).unwrap_or(0),
            })
        } else {
            None
        };
        let (scale, offset) = match sample_format {
            SampleFormat::Int16 => channel_scaling(
                instrument_scale,
                signal_gain,
                programmable_gain,
                telegraph
                    .as_ref()
                    .filter(|t| t.enabled)
                    .map(|t| t.additional_gain),
                adc_range,
                adc_resolution,
                instrument_offset,
                signal_offset,
            ),
            SampleFormat::Float32 => (1.0, 0.0),
        };
        channels.push(InputChannel {
            index: i as u32,
            adc_number: phys,
            name,
            unit,
            programmable_gain,
            instrument_scale,
            instrument_offset,
            signal_gain,
            signal_offset,
            lowpass_hz: lowpass,
            highpass_hz: highpass,
            telegraph,
            scale,
            offset,
        });
    }
    // Command outputs: names/units/holding for 4 DACs; epochs for the 2 waveform channels.
    let mut outputs = Vec::new();
    for d in 0..4usize {
        let mut epochs = Vec::new();
        let (enabled, source, stimulus) = if extended && d < 2 {
            for e in 0..10usize {
                let k = d * 10 + e;
                let code = arr_i16(2308, k).unwrap_or(0);
                if code == 0 {
                    continue;
                }
                epochs.push(Epoch {
                    index: e as u32,
                    kind: EpochKind::from_code(code),
                    kind_code: code,
                    level: arr_f32(2348, k).unwrap_or(0.0),
                    level_step: arr_f32(2428, k).unwrap_or(0.0),
                    duration: h.i32_at(2508 + 4 * k as u64).unwrap_or(0),
                    duration_step: h.i32_at(2588 + 4 * k as u64).unwrap_or(0),
                    pulse_period: h.i32_at(2136 + 4 * k as u64),
                    pulse_width: h.i32_at(2216 + 4 * k as u64),
                });
            }
            (
                arr_i16(2296, d) == Some(1),
                arr_i16(2300, d).unwrap_or(0),
                nonempty(h.text_at(2736 + 256 * d as u64, 256)),
            )
        } else if !extended && h.i16_at(1440).is_some_and(|a| a as usize == d) {
            for e in 0..10usize {
                let code = arr_i16(1444, e).unwrap_or(0);
                if code == 0 {
                    continue;
                }
                epochs.push(Epoch {
                    index: e as u32,
                    kind: EpochKind::from_code(code),
                    kind_code: code,
                    level: arr_f32(1464, e).unwrap_or(0.0),
                    level_step: arr_f32(1504, e).unwrap_or(0.0),
                    duration: i32::from(arr_i16(1544, e).unwrap_or(0)),
                    duration_step: i32::from(arr_i16(1564, e).unwrap_or(0)),
                    pulse_period: None,
                    pulse_width: None,
                });
            }
            (!epochs.is_empty(), i16::from(!epochs.is_empty()), None)
        } else {
            (false, 0, None)
        };
        outputs.push(OutputChannel {
            index: d as u32,
            name: h.text_at(1306 + 10 * d as u64, 10).unwrap_or_default(),
            unit: h.text_at(1346 + 8 * d as u64, 8).unwrap_or_default(),
            holding_level: arr_f32(1394, d).unwrap_or(0.0),
            waveform_enabled: enabled,
            waveform_source_code: source,
            stimulus_file: stimulus,
            epochs,
            inter_episode_last: extended && d < 2 && arr_i16(2304, d).is_some_and(|v| v != 0),
            conditioning: false,
        });
    }
    let digital = if h.i16_at(1436) == Some(1) {
        (0..10u32)
            .filter_map(|e| {
                arr_i16(1588, e as usize).map(|v| DigitalEpoch {
                    epoch: e,
                    pattern: v as u16,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let rate = if interval > 0.0 {
        1e6 / f64::from(interval) / f64::from(nch)
    } else {
        findings.push(Finding::error(
            "bad_sample_interval",
            format!("sample interval {interval} µs is not positive"),
        ));
        0.0
    };
    let mode = AcquisitionMode::from_code(mode_code);
    let sweep_count = if mode == AcquisitionMode::GapFree || episodes <= 0 {
        1
    } else {
        episodes as u64
    };
    let synch = if synch_block > 0 && synch_count > 0 {
        let b = read_records(
            f,
            path,
            synch_block as u64 * BLOCK_LEN,
            synch_count as u64,
            8,
            file_len,
        )?;
        read_synch(&b, synch_count as u64, 8)
    } else {
        Vec::new()
    };
    let sweep_interval = (episode_interval > 0.0 && episode_interval.is_finite())
        .then_some(f64::from(episode_interval));
    let sweeps = layout_sweeps(
        u64::try_from(acq_len).unwrap_or(0),
        nch as u64,
        sweep_count,
        (&synch, synch_unit),
        rate,
        sweep_interval,
        &mut findings,
    );
    let tag_unit = synch_unit;
    let tags = if tag_block > 0 && tag_count > 0 {
        let b = read_records(
            f,
            path,
            tag_block as u64 * BLOCK_LEN,
            tag_count as u64,
            64,
            file_len,
        )?;
        read_tags(&b, tag_count as u64, 64, |t| {
            if tag_unit > 0.0 {
                f64::from(t) * f64::from(tag_unit) / 1e6
            } else {
                f64::from(t) * f64::from(interval) / 1e6
            }
        })
    } else {
        Vec::new()
    };
    let ms = if extended || h.end() > 368 {
        i64::from(h.i16_at(366).unwrap_or(0).max(0))
    } else {
        0
    };
    let created_at = iso_datetime(i64::from(date), i64::from(time_s) * 1000 + ms);
    if created_at.is_none() {
        findings.push(Finding::info(
            "no_creation_time",
            format!("recording date {date} / time {time_s} s is not a valid date"),
        ));
    }
    let creator = nonempty(h.text_at(294, 16));
    let creator_version = if extended {
        let v: Vec<i16> = (0..4).filter_map(|i| h.i16_at(5798 + 2 * i)).collect();
        (v.len() == 4 && v.iter().any(|&x| x != 0))
            .then(|| format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3]))
    } else {
        None
    };
    let comment = if extended {
        nonempty(h.text_at(5154, 128)).or_else(|| nonempty(h.text_at(310, 56)))
    } else {
        nonempty(h.text_at(310, 56))
    };
    let mut sections = vec![SectionEntry {
        name: "header",
        block: 0,
        entry_size: header_len as u32,
        entry_count: 1,
        offset: 0,
        byte_len: header_len,
    }];
    let total = u64::try_from(acq_len).unwrap_or(0);
    sections.push(SectionEntry {
        name: "data",
        block: data_block as u32,
        entry_size: sample_format.width() as u32,
        entry_count: total,
        offset: data_offset,
        byte_len: total.saturating_mul(sample_format.width()),
    });
    if synch_block > 0 && synch_count > 0 {
        sections.push(SectionEntry {
            name: "synch_array",
            block: synch_block as u32,
            entry_size: 8,
            entry_count: synch_count as u64,
            offset: synch_block as u64 * BLOCK_LEN,
            byte_len: synch_count as u64 * 8,
        });
    }
    if tag_block > 0 && tag_count > 0 {
        sections.push(SectionEntry {
            name: "tag",
            block: tag_block as u32,
            entry_size: 64,
            entry_count: tag_count as u64,
            offset: tag_block as u64 * BLOCK_LEN,
            byte_len: tag_count as u64 * 64,
        });
    }
    Ok(AbfFile {
        generation: Generation::Abf1,
        version: format!("{version_f:.2}"),
        file_len,
        header_len,
        mode,
        sample_format,
        data_offset,
        total_samples: total,
        sample_interval_us: f64::from(interval) * f64::from(nch),
        sample_rate_hz: rate,
        header_sweep_count: i64::from(episodes),
        sweeps,
        channels,
        outputs,
        digital,
        adc_range_v: adc_range,
        adc_resolution,
        synch_time_unit_us: synch_unit,
        sweep_interval_s: sweep_interval,
        creator,
        creator_version,
        protocol_path: if extended {
            nonempty(h.text_at(4898, 256))
        } else {
            None
        },
        comment,
        created_at,
        guid: if extended {
            h.slice(5282, 16).and_then(guid_text)
        } else {
            None
        },
        experiment_kind_code: h.i16_at(260),
        digitizer_code: None,
        tags,
        synch,
        sections,
        strings: Vec::new(),
        user_list_active: false,
        alternate_dac_output: false,
        findings,
    })
}

// ---------------------------------------------------------------- ABF 2

#[allow(clippy::many_single_char_names)]
fn parse_abf2(f: &mut SourceFile, path: &Path, head: &Block, file_len: u64) -> Result<AbfFile> {
    let h = head;
    let mut findings = Vec::new();
    let map_end = SECTION_MAP_OFFSET + SECTION_ENTRY_LEN * SECTION_NAMES.len() as u64;
    if h.end() < map_end {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            h.end(),
            format!(
                "file ends at byte {} before the end of the section map ({map_end})",
                h.end()
            ),
        ));
    }
    let v = h.slice(4, 4).unwrap_or(&[0, 0, 0, 0]);
    let version = format!("{}.{}.{}.{}", v[3], v[2], v[1], v[0]);
    let episodes = h.u32_at(12).unwrap_or(0);
    let date = h.u32_at(16).unwrap_or(0);
    let time_ms = h.u32_at(20).unwrap_or(0);
    let data_format = h.u16_at(30).unwrap_or(0);
    let cv = h.slice(56, 4).unwrap_or(&[0, 0, 0, 0]);
    let creator_version = cv
        .iter()
        .any(|&x| x != 0)
        .then(|| format!("{}.{}.{}.{}", cv[3], cv[2], cv[1], cv[0]));
    let creator_index = h.u32_at(60).unwrap_or(0);
    let protocol_index = h.u32_at(72).unwrap_or(0);
    let mut sections = vec![SectionEntry {
        name: "header",
        block: 0,
        entry_size: 512,
        entry_count: 1,
        offset: 0,
        byte_len: 512.min(file_len),
    }];
    let mut by_name = std::collections::HashMap::new();
    for (i, name) in SECTION_NAMES.iter().enumerate() {
        let at = SECTION_MAP_OFFSET + SECTION_ENTRY_LEN * i as u64;
        let block = h.u32_at(at).unwrap_or(0);
        let size = h.u32_at(at + 4).unwrap_or(0);
        let count = h.i64_at(at + 8).unwrap_or(0);
        if block == 0 && count == 0 {
            continue;
        }
        let count_u = u64::try_from(count).unwrap_or(0);
        if count < 0 {
            findings.push(
                Finding::error(
                    "bad_section",
                    format!("section {name}: negative entry count {count}"),
                )
                .at(at),
            );
        }
        let offset = u64::from(block) * BLOCK_LEN;
        let byte_len = if *name == "strings" {
            u64::from(size)
        } else {
            u64::from(size).saturating_mul(count_u)
        };
        let e = SectionEntry {
            name,
            block,
            entry_size: size,
            entry_count: count_u,
            offset,
            byte_len,
        };
        by_name.insert(*name, e.clone());
        sections.push(e);
    }
    let section = |n: &str| by_name.get(n).cloned();
    let read_section = |f: &mut SourceFile, s: &SectionEntry, cap: u64| -> Result<Block> {
        read_block(f, path, s.offset, s.byte_len.min(cap), file_len)
    };
    // protocol
    let proto = section("protocol").ok_or_else(|| {
        Error::corrupt_at(
            FORMAT_ID,
            SECTION_MAP_OFFSET,
            "the section map has no protocol section",
        )
    })?;
    let p = read_section(f, &proto, 4096)?;
    let po = proto.offset;
    let mode_code = p.i16_at(po).unwrap_or(0);
    let interval = p.f32_at(po + 2).unwrap_or(0.0);
    let synch_unit = p.f32_at(po + 14).unwrap_or(0.0);
    let episode_interval = p.f32_at(po + 62).unwrap_or(0.0);
    let adc_range = p.f32_at(po + 110).unwrap_or(0.0);
    let adc_resolution = p.i32_at(po + 118).unwrap_or(0);
    let experiment = p.i16_at(po + 126);
    let comment_index = p.i32_at(po + 132).unwrap_or(0);
    let digitizer = p.i16_at(po + 206);
    let alternate_dac_output = p.i16_at(po + 182).is_some_and(|v| v != 0);
    if p.end() < po + 210 {
        findings.push(Finding::error(
            "truncated",
            "the protocol section is cut off by the end of the file",
        ));
    }
    // strings
    let strings = match section("strings") {
        Some(s) => parse_string_table(&read_section(f, &s, MAX_STRINGS_LEN)?.bytes),
        None => vec![String::new()],
    };
    let string = |i: i64| -> String {
        usize::try_from(i)
            .ok()
            .and_then(|i| strings.get(i))
            .cloned()
            .unwrap_or_default()
    };
    // data
    let data = section("data").ok_or_else(|| {
        Error::corrupt_at(
            FORMAT_ID,
            SECTION_MAP_OFFSET,
            "the section map has no data section",
        )
    })?;
    let sample_format = match (data_format, data.entry_size) {
        (0, 2) => SampleFormat::Int16,
        (1, 4) => SampleFormat::Float32,
        (fmt, size) => {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                30,
                format!(
                    "data format code {fmt} with {size}-byte samples is not int16 (0, 2) or float32 (1, 4)"
                ),
            ));
        }
    };
    // ADC
    let adc = section("adc").ok_or_else(|| {
        Error::corrupt_at(
            FORMAT_ID,
            SECTION_MAP_OFFSET,
            "the section map has no ADC section",
        )
    })?;
    if adc.entry_count == 0 || adc.entry_count > 64 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            adc.offset,
            format!("ADC section lists {} channels", adc.entry_count),
        ));
    }
    let a = read_section(f, &adc, 64 * 1024)?;
    let mut channels = Vec::new();
    for i in 0..adc.entry_count {
        let at = adc.offset + i * u64::from(adc.entry_size);
        let (Some(adc_number), Some(name_index)) = (a.i16_at(at), a.i32_at(at + 74)) else {
            findings.push(
                Finding::error(
                    "truncated",
                    format!("ADC entry {i} is cut off by the end of the file"),
                )
                .at(at),
            );
            break;
        };
        let f32v = |off: u64, d: f32| a.f32_at(at + off).unwrap_or(d);
        let telegraph = Telegraph {
            enabled: a.i16_at(at + 2) == Some(1),
            instrument_code: a.i16_at(at + 4).unwrap_or(0),
            additional_gain: f32v(6, 1.0),
            filter_hz: f32v(10, 0.0),
            membrane_capacitance: f32v(14, 0.0),
            clamp_mode_code: a.i16_at(at + 18).unwrap_or(0),
        };
        let programmable_gain = f32v(28, 1.0);
        let instrument_scale = f32v(40, 1.0);
        let instrument_offset = f32v(44, 0.0);
        let signal_gain = f32v(48, 1.0);
        let signal_offset = f32v(52, 0.0);
        let (scale, offset) = match sample_format {
            SampleFormat::Int16 => channel_scaling(
                instrument_scale,
                signal_gain,
                programmable_gain,
                telegraph.enabled.then_some(telegraph.additional_gain),
                adc_range,
                adc_resolution,
                instrument_offset,
                signal_offset,
            ),
            SampleFormat::Float32 => (1.0, 0.0),
        };
        channels.push(InputChannel {
            index: i as u32,
            adc_number,
            name: string(i64::from(name_index)),
            unit: string(i64::from(a.i32_at(at + 78).unwrap_or(0))),
            programmable_gain,
            instrument_scale,
            instrument_offset,
            signal_gain,
            signal_offset,
            lowpass_hz: a.f32_at(at + 56),
            highpass_hz: a.f32_at(at + 60),
            telegraph: Some(telegraph),
            scale,
            offset,
        });
    }
    if channels.is_empty() {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            adc.offset,
            "no readable ADC channel entries",
        ));
    }
    // epochs per DAC and digital epochs
    let mut epochs_by_dac: std::collections::BTreeMap<i16, Vec<Epoch>> = Default::default();
    if let Some(s) = section("epoch_per_dac") {
        let b = read_section(f, &s, 1 << 20)?;
        for i in 0..s.entry_count.min(MAX_RECORDS) {
            let at = s.offset + i * u64::from(s.entry_size);
            let (Some(num), Some(dac), Some(code)) =
                (b.i16_at(at), b.i16_at(at + 2), b.i16_at(at + 4))
            else {
                break;
            };
            epochs_by_dac.entry(dac).or_default().push(Epoch {
                index: u32::try_from(num.max(0)).unwrap_or(0),
                kind: EpochKind::from_code(code),
                kind_code: code,
                level: b.f32_at(at + 6).unwrap_or(0.0),
                level_step: b.f32_at(at + 10).unwrap_or(0.0),
                duration: b.i32_at(at + 14).unwrap_or(0),
                duration_step: b.i32_at(at + 18).unwrap_or(0),
                pulse_period: b.i32_at(at + 22),
                pulse_width: b.i32_at(at + 26),
            });
        }
    }
    let mut digital = Vec::new();
    if let Some(s) = section("epoch") {
        let b = read_section(f, &s, 1 << 20)?;
        for i in 0..s.entry_count.min(MAX_RECORDS) {
            let at = s.offset + i * u64::from(s.entry_size);
            let (Some(num), Some(pattern)) = (b.i16_at(at), b.i16_at(at + 2)) else {
                break;
            };
            digital.push(DigitalEpoch {
                epoch: u32::try_from(num.max(0)).unwrap_or(0),
                pattern: pattern as u16,
            });
        }
    }
    // DAC
    let mut outputs = Vec::new();
    if let Some(s) = section("dac") {
        let b = read_section(f, &s, 1 << 20)?;
        for i in 0..s.entry_count.min(64) {
            let at = s.offset + i * u64::from(s.entry_size);
            let Some(num) = b.i16_at(at) else { break };
            let enabled = b.i16_at(at + 40) == Some(1);
            outputs.push(OutputChannel {
                index: u32::try_from(num.max(0)).unwrap_or(i as u32),
                name: string(i64::from(b.i32_at(at + 24).unwrap_or(0))),
                unit: string(i64::from(b.i32_at(at + 28).unwrap_or(0))),
                holding_level: b.f32_at(at + 12).unwrap_or(0.0),
                waveform_enabled: enabled,
                waveform_source_code: b.i16_at(at + 42).unwrap_or(0),
                stimulus_file: nonempty(Some(string(i64::from(b.i32_at(at + 118).unwrap_or(0))))),
                epochs: epochs_by_dac.remove(&num).unwrap_or_default(),
                inter_episode_last: b.i16_at(at + 44).is_some_and(|v| v != 0),
                conditioning: b.i16_at(at + 60).is_some_and(|v| v != 0),
            });
        }
    }
    // user list: an entry that is enabled or names a parameter to vary
    let mut user_list_active = false;
    if let Some(s) = section("user_list") {
        let b = read_section(f, &s, 1 << 20)?;
        for i in 0..s.entry_count.min(64) {
            let at = s.offset + i * u64::from(s.entry_size);
            let (Some(enable), Some(param)) = (b.i16_at(at + 2), b.i16_at(at + 4)) else {
                break;
            };
            user_list_active |= enable != 0 || param > 0;
        }
    }
    let rate = if interval > 0.0 {
        1e6 / f64::from(interval)
    } else {
        findings.push(Finding::error(
            "bad_sample_interval",
            format!("sample interval {interval} µs is not positive"),
        ));
        0.0
    };
    let mode = AcquisitionMode::from_code(mode_code);
    let sweep_count = if mode == AcquisitionMode::GapFree || episodes == 0 {
        1
    } else {
        u64::from(episodes)
    };
    let synch = match section("synch_array") {
        Some(s) if s.entry_count > 0 => {
            let b = read_records(f, path, s.offset, s.entry_count, 8, file_len)?;
            read_synch(&b, s.entry_count.min(MAX_RECORDS), u64::from(s.entry_size))
        }
        _ => Vec::new(),
    };
    let nch = channels.len() as u64;
    let sweep_interval = (episode_interval > 0.0 && episode_interval.is_finite())
        .then_some(f64::from(episode_interval));
    let sweeps = layout_sweeps(
        data.entry_count,
        nch,
        sweep_count,
        (&synch, synch_unit),
        rate,
        sweep_interval,
        &mut findings,
    );
    let tags = match section("tag") {
        Some(s) if s.entry_count > 0 => {
            let b = read_records(
                f,
                path,
                s.offset,
                s.entry_count,
                u64::from(s.entry_size),
                file_len,
            )?;
            read_tags(
                &b,
                s.entry_count.min(MAX_RECORDS),
                u64::from(s.entry_size),
                |t| {
                    if synch_unit > 0.0 {
                        f64::from(t) * f64::from(synch_unit) / 1e6
                    } else if rate > 0.0 {
                        f64::from(t) / rate / nch as f64
                    } else {
                        0.0
                    }
                },
            )
        }
        _ => Vec::new(),
    };
    let created_at = iso_datetime(i64::from(date), i64::from(time_ms));
    if created_at.is_none() {
        findings.push(Finding::info(
            "no_creation_time",
            format!("recording date {date} / time {time_ms} ms is not a valid date"),
        ));
    }
    Ok(AbfFile {
        generation: Generation::Abf2,
        version,
        file_len,
        header_len: 512,
        mode,
        sample_format,
        data_offset: data.offset,
        total_samples: data.entry_count,
        sample_interval_us: f64::from(interval),
        sample_rate_hz: rate,
        header_sweep_count: i64::from(episodes),
        sweeps,
        channels,
        outputs,
        digital,
        adc_range_v: adc_range,
        adc_resolution,
        synch_time_unit_us: synch_unit,
        sweep_interval_s: sweep_interval,
        creator: nonempty(Some(string(i64::from(creator_index)))),
        creator_version,
        protocol_path: nonempty(Some(string(i64::from(protocol_index)))),
        comment: nonempty(Some(string(i64::from(comment_index)))),
        created_at,
        guid: h.slice(40, 16).and_then(guid_text),
        experiment_kind_code: experiment,
        digitizer_code: digitizer,
        tags,
        synch,
        sections,
        strings,
        user_list_active,
        alternate_dac_output,
        findings,
    })
}

/// True when `v` is usable as a divisor in the scaling chain (finite and non-zero).
pub fn usable_factor(v: f32) -> bool {
    v.is_finite() && v != 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            iso_datetime(20_141_016, 36_095_078).as_deref(),
            Some("2014-10-16T10:01:35.078")
        );
        assert_eq!(
            iso_datetime(180_618, 63_267_000).as_deref(),
            Some("2018-06-18T17:34:27.000")
        );
        assert_eq!(
            iso_datetime(990_101, 0).as_deref(),
            Some("1999-01-01T00:00:00.000")
        );
        assert_eq!(iso_datetime(-1, -1000), None);
        assert_eq!(iso_datetime(20_181_340, 0), None);
    }

    #[test]
    fn strings() {
        let mut b = b"SSCH\x01\x00\x00\x00\x03\x00\x00\x00".to_vec();
        b.extend_from_slice(&[0u8; 28]);
        b.extend_from_slice(b"Clampex\x00C:\\p\\x.pro\x00\xb5A\x00\x00\x00");
        let t = parse_string_table(&b);
        assert_eq!(t, vec!["", "Clampex", "C:\\p\\x.pro", "µA"]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn scaling_order() {
        let (g, o) = channel_scaling(0.001, 1.0, 1.0, Some(5.0), 10.0, 32768, 0.5, 0.25);
        assert_eq!(g, 1.0 / 0.001_f32 as f64 / 1.0 / 1.0 / 5.0 * 10.0 / 32768.0);
        assert!((o - 0.25).abs() < 1e-12);
    }

    #[test]
    fn guid() {
        let b: Vec<u8> = (0u8..16).collect();
        assert_eq!(
            guid_text(&b).as_deref(),
            Some("03020100-0504-0706-0809-0A0B0C0D0E0F")
        );
        assert_eq!(guid_text(&[0; 16]), None);
    }
}
