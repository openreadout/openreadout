//! NEV event files: basic header, extended headers, fixed-size data packets.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{Block, latin1_field, read_block, utf16le_z};
use openreadout_core::model::Finding;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::nsx::systemtime;

/// Bytes of the NEV basic header.
pub const NEV_BASIC_LEN: u64 = 336;
/// Bytes of one NEV extended header.
pub const NEV_EXT_LEN: u64 = 32;
/// Packet id of comment packets.
pub const COMMENT_PACKET: u16 = 0xFFFF;
/// Highest packet id that names an electrode (spike packets).
pub const MAX_ELECTRODE_ID: u16 = 32_767;

/// Spike-waveform settings of one electrode (`NEUEVWAV`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveformHeader {
    pub electrode_id: u16,
    pub connector: u8,
    pub pin: u8,
    /// nV per count.
    pub digitization_nv: u16,
    pub energy_threshold: u16,
    pub high_threshold_uv: i16,
    pub low_threshold_uv: i16,
    pub sorted_units: u8,
    /// Bytes per waveform sample (0 or 1 → 1).
    pub sample_bytes: u8,
    /// Samples per waveform (spec 2.3+; `None` before).
    pub spike_width: Option<u16>,
}

/// One extended header kept verbatim (id plus 24 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtHeader {
    pub id: String,
    pub offset: u64,
}

/// A parsed NEV header.
#[derive(Debug, Clone)]
pub struct NevFile {
    /// `NEURALEV` (spec ≤ 2.3) or `BREVENTS` (3.0).
    pub signature: String,
    pub version: (u8, u8),
    pub flags: u16,
    pub header_len: u64,
    pub packet_len: u64,
    pub time_resolution: u32,
    pub sample_resolution: u32,
    pub recorded_at: Option<String>,
    pub application: String,
    pub comment: String,
    pub ext_headers: Vec<ExtHeader>,
    pub waveforms: BTreeMap<u16, WaveformHeader>,
    pub labels: BTreeMap<u16, String>,
    /// `DIGLABEL`: (label, mode 0 serial / 1 parallel).
    pub digital_labels: Vec<(String, u8)>,
    pub file_len: u64,
    pub findings: Vec<Finding>,
}

impl NevFile {
    /// 64-bit timestamps (spec 3.0).
    pub fn wide_timestamps(&self) -> bool {
        self.version.0 >= 3
    }
    /// Offset of the payload after timestamp and packet id.
    pub fn payload_offset(&self) -> u64 {
        if self.wide_timestamps() { 10 } else { 6 }
    }
    /// Offset of the spike waveform in a packet.
    pub fn waveform_offset(&self) -> u64 {
        self.payload_offset() + 2
    }
    pub fn packet_count(&self) -> u64 {
        self.file_len
            .saturating_sub(self.header_len)
            .checked_div(self.packet_len)
            .unwrap_or(0)
    }
    /// Bytes per waveform sample of an electrode (flag bit 0 forces 2).
    pub fn sample_bytes(&self, electrode: u16) -> u64 {
        if self.flags & 1 == 1 {
            return 2;
        }
        match self.waveforms.get(&electrode).map(|w| w.sample_bytes) {
            Some(0 | 1) | None => 1,
            Some(b) => u64::from(b),
        }
    }
    /// Samples per waveform of an electrode.
    pub fn waveform_samples(&self, electrode: u16) -> u64 {
        let room =
            self.packet_len.saturating_sub(self.waveform_offset()) / self.sample_bytes(electrode);
        match self.waveforms.get(&electrode).and_then(|w| w.spike_width) {
            Some(w) if w > 0 => u64::from(w).min(room),
            _ => room,
        }
    }
    /// Longest waveform of any electrode (table width).
    pub fn max_waveform_samples(&self) -> u64 {
        let base = self.packet_len.saturating_sub(self.waveform_offset());
        self.waveforms
            .keys()
            .map(|&e| self.waveform_samples(e))
            .max()
            .unwrap_or(base / if self.flags & 1 == 1 { 2 } else { 1 })
    }
    /// µV per count of an electrode's waveform.
    pub fn uv_per_count(&self, electrode: u16) -> Option<f64> {
        self.waveforms
            .get(&electrode)
            .map(|w| crate::nsx::spec21_factor(w.digitization_nv) / 1000.0)
    }
}

/// Parse the basic and extended headers of a NEV file.
pub fn parse_nev(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<NevFile> {
    let head = read_block(f, path, 0, NEV_BASIC_LEN, file_len)?;
    let sig = head.slice(0, 8).unwrap_or(&[]);
    if sig != b"NEURALEV" && sig != b"BREVENTS" {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            0,
            "no NEV signature (`NEURALEV`, `BREVENTS`) at byte 0",
        ));
    }
    if head.len() < NEV_BASIC_LEN as usize {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            file_len,
            format!("file ends inside the {NEV_BASIC_LEN}-byte basic header"),
        ));
    }
    let mut findings = Vec::new();
    let header_len = u64::from(head.u32_at(12).unwrap_or(0));
    let packet_len = u64::from(head.u32_at(16).unwrap_or(0));
    let n_ext = u64::from(head.u32_at(332).unwrap_or(0));
    if header_len != NEV_BASIC_LEN + NEV_EXT_LEN * n_ext {
        findings.push(Finding::warning(
            "header_length_mismatch",
            format!(
                "header says {header_len} bytes; 336 + 32 × {n_ext} extended headers = {}",
                NEV_BASIC_LEN + NEV_EXT_LEN * n_ext
            ),
        ));
    }
    if !(8..=1 << 20).contains(&packet_len) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            16,
            format!("data packet size {packet_len} is not plausible"),
        ));
    }
    let mut nev = NevFile {
        signature: String::from_utf8_lossy(sig).into_owned(),
        version: (head.u8_at(8).unwrap_or(0), head.u8_at(9).unwrap_or(0)),
        flags: head.u16_at(10).unwrap_or(0),
        header_len,
        packet_len,
        time_resolution: head.u32_at(20).unwrap_or(0),
        sample_resolution: head.u32_at(24).unwrap_or(0),
        recorded_at: systemtime(&head, 28),
        application: head.text_at(44, 32).unwrap_or_default(),
        comment: head.text_at(76, 256).unwrap_or_default(),
        ext_headers: Vec::new(),
        waveforms: BTreeMap::new(),
        labels: BTreeMap::new(),
        digital_labels: Vec::new(),
        file_len,
        findings: Vec::new(),
    };
    let ext = read_block(
        f,
        path,
        NEV_BASIC_LEN,
        n_ext.min(1 << 20) * NEV_EXT_LEN,
        file_len,
    )?;
    let spec23 = nev.version >= (2, 3);
    for i in 0..n_ext.min(1 << 20) {
        let at = NEV_BASIC_LEN + i * NEV_EXT_LEN;
        let Some(id_bytes) = ext.slice(at, 8) else {
            findings.push(
                Finding::error("truncated", "the file ends inside the extended headers").at(at),
            );
            break;
        };
        let id = latin1_field(id_bytes);
        match id.as_str() {
            "NEUEVWAV" => {
                let e = ext.u16_at(at + 8).unwrap_or(0);
                nev.waveforms.insert(
                    e,
                    WaveformHeader {
                        electrode_id: e,
                        connector: ext.u8_at(at + 10).unwrap_or(0),
                        pin: ext.u8_at(at + 11).unwrap_or(0),
                        digitization_nv: ext.u16_at(at + 12).unwrap_or(0),
                        energy_threshold: ext.u16_at(at + 14).unwrap_or(0),
                        high_threshold_uv: ext.i16_at(at + 16).unwrap_or(0),
                        low_threshold_uv: ext.i16_at(at + 18).unwrap_or(0),
                        sorted_units: ext.u8_at(at + 20).unwrap_or(0),
                        sample_bytes: ext.u8_at(at + 21).unwrap_or(0),
                        spike_width: if spec23 { ext.u16_at(at + 22) } else { None },
                    },
                );
            }
            "NEUEVLBL" => {
                let e = ext.u16_at(at + 8).unwrap_or(0);
                nev.labels
                    .insert(e, ext.text_at(at + 10, 16).unwrap_or_default());
            }
            "DIGLABEL" => {
                nev.digital_labels.push((
                    ext.text_at(at + 8, 16).unwrap_or_default(),
                    ext.u8_at(at + 24).unwrap_or(0),
                ));
            }
            _ => {}
        }
        nev.ext_headers.push(ExtHeader { id, offset: at });
    }
    let data = file_len.saturating_sub(header_len);
    if header_len > file_len {
        findings.push(Finding::error("truncated", "the file ends inside the headers").at(file_len));
    } else if !data.is_multiple_of(packet_len) {
        findings.push(
            Finding::warning(
                "partial_packet",
                format!(
                    "{} bytes after the last whole data packet are ignored",
                    data % packet_len
                ),
            )
            .at(header_len + (data / packet_len) * packet_len),
        );
    }
    nev.findings = findings;
    Ok(nev)
}

/// One data packet, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct NevPacket {
    pub timestamp: u64,
    pub packet_id: u16,
    /// Spike: unit class; digital: insertion reason; comment: character set.
    pub code: u8,
    /// Digital: input value.
    pub digital: Option<u16>,
    /// Spike: raw waveform samples.
    pub waveform: Vec<i32>,
    /// Comment: text.
    pub text: Option<String>,
}

/// Decode the packet at `at`.
pub fn nev_packet(nev: &NevFile, b: &Block, at: u64) -> Option<NevPacket> {
    let (ts, id) = if nev.wide_timestamps() {
        (b.u64_at(at)?, b.u16_at(at + 8)?)
    } else {
        (u64::from(b.u32_at(at)?), b.u16_at(at + 4)?)
    };
    let payload = at + nev.payload_offset();
    let mut pk = NevPacket {
        timestamp: ts,
        packet_id: id,
        code: b.u8_at(payload)?,
        digital: None,
        waveform: Vec::new(),
        text: None,
    };
    match id {
        0 => pk.digital = b.u16_at(payload + 2),
        COMMENT_PACKET => {
            let text_at = payload + 6;
            let len = (at + nev.packet_len).saturating_sub(text_at) as usize;
            let raw = b.slice(text_at, len)?;
            pk.text = Some(if pk.code == 1 {
                utf16le_z(raw)
            } else {
                latin1_field(raw)
            });
        }
        e if (1..=MAX_ELECTRODE_ID).contains(&e) => {
            let width = nev.sample_bytes(e);
            let samples = nev.waveform_samples(e);
            let base = at + nev.waveform_offset();
            for i in 0..samples {
                let value = match width {
                    1 => i32::from(b.i8_at(base + i)?),
                    2 => i32::from(b.i16_at(base + 2 * i)?),
                    _ => b.i32_at(base + 4 * i)?,
                };
                pk.waveform.push(value);
            }
        }
        _ => {}
    }
    Some(pk)
}
