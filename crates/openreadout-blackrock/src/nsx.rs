//! NSx continuous files: spec 2.1 (`NEURALSG`), 2.2/2.3 (`NEURALCD`), 3.0 (`BRSMPGRP`, 64-bit
//! timestamps, PTP one-sample packets).

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{Block, read_block};
use openreadout_core::model::Finding;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Clock of the `period` field (ticks per second).
pub const PERIOD_CLOCK_HZ: f64 = 30_000.0;
/// Bytes of the spec 2.2+ basic header.
pub const BASIC_HEADER_LEN: u64 = 314;
/// Bytes of one `CC` extended header.
pub const CC_LEN: u64 = 66;
/// Bytes of the spec 2.1 basic header (before the electrode ids).
pub const SPEC21_HEADER_LEN: u64 = 32;
/// Timestamp resolution of PTP files (nanoseconds).
pub const PTP_RESOLUTION: u32 = 1_000_000_000;
/// Most channels accepted (guards allocation on a damaged header).
pub const MAX_CHANNELS: u32 = 65_535;

/// Which header layout the file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsxSpec {
    /// `NEURALSG`: no per-channel headers, no packets, no timestamps.
    V21,
    /// `NEURALCD`: `CC` headers, packets with 32-bit timestamps.
    V22,
    /// `BRSMPGRP`: packets with 64-bit timestamps.
    V30,
}

impl NsxSpec {
    pub fn name(self) -> &'static str {
        match self {
            NsxSpec::V21 => "2.1",
            NsxSpec::V22 => "2.2/2.3",
            NsxSpec::V30 => "3.0",
        }
    }
    /// Bytes of a data-packet header (0x01, timestamp, point count).
    pub fn packet_header_len(self) -> u64 {
        match self {
            NsxSpec::V21 => 0,
            NsxSpec::V22 => 9,
            NsxSpec::V30 => 13,
        }
    }
}

/// One channel (`CC` extended header, or an electrode id in spec 2.1).
#[derive(Debug, Clone, PartialEq)]
pub struct NsxChannel {
    pub index: u32,
    pub electrode_id: u16,
    pub label: String,
    pub connector: Option<u8>,
    pub pin: Option<u8>,
    pub min_digital: Option<i16>,
    pub max_digital: Option<i16>,
    pub min_analog: Option<i16>,
    pub max_analog: Option<i16>,
    pub units: String,
    /// High-pass corner (mHz), order, type (0 none, 1 Butterworth, 2 Chebyshev).
    pub highpass: Option<(u32, u32, u16)>,
    /// Low-pass corner (mHz), order, type.
    pub lowpass: Option<(u32, u32, u16)>,
    /// `value = raw × scale + offset`.
    pub scale: f64,
    pub offset: f64,
}

/// A data packet: its header offset, first sample offset, timestamp and point count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NsxPacket {
    pub offset: u64,
    pub data_offset: u64,
    pub timestamp: u64,
    pub points: u64,
}

/// A gap-free run of samples: one packet, or a run of one-sample PTP packets.
#[derive(Debug, Clone, PartialEq)]
pub struct NsxSweep {
    pub first_packet: u64,
    pub packet_count: u64,
    pub sample_count: u64,
    pub timestamp: u64,
}

/// A parsed NSx header plus its packet index.
#[derive(Debug, Clone)]
pub struct NsxFile {
    pub spec: NsxSpec,
    pub version: Option<(u8, u8)>,
    pub label: String,
    pub comment: String,
    pub period: u32,
    pub time_resolution: u32,
    pub recorded_at: Option<String>,
    pub header_len: u64,
    pub channels: Vec<NsxChannel>,
    pub packets: Vec<NsxPacket>,
    pub sweeps: Vec<NsxSweep>,
    /// One sample per packet (PTP): bytes from one packet to the next.
    pub ptp_stride: Option<u64>,
    pub ptp_packet_count: u64,
    /// Where spec 2.1 scaling came from (the companion NEV path), if anywhere.
    pub scaling_source: Option<String>,
    pub file_len: u64,
    pub findings: Vec<Finding>,
}

impl NsxFile {
    pub fn sample_rate_hz(&self) -> f64 {
        if self.period == 0 {
            0.0
        } else {
            PERIOD_CLOCK_HZ / f64::from(self.period)
        }
    }
    /// Bytes of one sample of every channel.
    pub fn frame_len(&self) -> u64 {
        2 * self.channels.len() as u64
    }
    /// File offset of sample `i` (per channel) of sweep `s`.
    pub fn sample_offset(&self, s: &NsxSweep, i: u64) -> Option<u64> {
        if let Some(stride) = self.ptp_stride {
            let first = self.packets.first()?.offset;
            first
                .checked_add((s.first_packet + i).checked_mul(stride)?)?
                .checked_add(self.spec.packet_header_len())
        } else {
            let p = self.packets.get(s.first_packet as usize)?;
            p.data_offset.checked_add(i.checked_mul(self.frame_len())?)
        }
    }
    pub fn max_sweep_len(&self) -> u64 {
        self.sweeps
            .iter()
            .map(|s| s.sample_count)
            .max()
            .unwrap_or(0)
    }
}

/// Windows SYSTEMTIME (8 × u16: year, month, weekday, day, hour, minute, second, ms), UTC.
pub fn systemtime(b: &Block, at: u64) -> Option<String> {
    let v: Vec<u16> = (0..8)
        .map(|i| b.u16_at(at + 2 * i))
        .collect::<Option<_>>()?;
    let (year, month, day, hour, minute, second, ms) = (v[0], v[1], v[3], v[4], v[5], v[6], v[7]);
    if !(1970..=2200).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || ms > 999
    {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z"
    ))
}

/// `(max_analog − min_analog) / (max_digital − min_digital)` and `min_analog − min_digital × scale`.
pub fn cc_scaling(min_d: i16, max_d: i16, min_a: i16, max_a: i16) -> Option<(f64, f64)> {
    if max_d == min_d {
        return None;
    }
    let scale = (f64::from(max_a) - f64::from(min_a)) / (f64::from(max_d) - f64::from(min_d));
    let offset = f64::from(min_a) - f64::from(min_d) * scale;
    Some((scale, offset))
}

/// Spec 2.1 channel names: `chan<id>` for front-end electrodes, `ainp<id − 128>` for analog inputs.
pub fn spec21_label(id: u16) -> String {
    if id < 129 {
        format!("chan{id}")
    } else {
        format!("ainp{}", id - 128)
    }
}

/// Digitization factor (nV per count) of a spec 2.1 electrode from the companion NEV, with the
/// replacement Neo documents for the 21516 overflow of old Cerebus systems.
pub fn spec21_factor(raw: u16) -> f64 {
    if raw == 21516 {
        152_592.547
    } else {
        f64::from(raw)
    }
}

/// Parse an NSx file: header, channels, packet index, sweeps. `nev_factors` gives spec 2.1
/// scaling (electrode id → nV per count) when a companion NEV was found.
pub fn parse_nsx(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
    nev_factors: Option<(String, BTreeMap<u16, u16>)>,
    full_scan: bool,
) -> Result<NsxFile> {
    let head = read_block(f, path, 0, 1 << 16, file_len)?;
    let sig = head.slice(0, 8).unwrap_or(&[]);
    let spec = match sig {
        b"NEURALSG" => NsxSpec::V21,
        b"NEURALCD" => NsxSpec::V22,
        b"BRSMPGRP" => NsxSpec::V30,
        _ => {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                0,
                "no NSx signature (`NEURALSG`, `NEURALCD`, `BRSMPGRP`) at byte 0",
            ));
        }
    };
    let short = |what: &str| {
        Error::corrupt(
            FORMAT_ID,
            format!("header ends before {what} (file is {file_len} bytes)"),
        )
    };
    let mut findings = Vec::new();
    let mut nsx = NsxFile {
        spec,
        version: None,
        label: String::new(),
        comment: String::new(),
        period: 0,
        time_resolution: 30_000,
        recorded_at: None,
        header_len: 0,
        channels: Vec::new(),
        packets: Vec::new(),
        sweeps: Vec::new(),
        ptp_stride: None,
        ptp_packet_count: 0,
        scaling_source: None,
        file_len,
        findings: Vec::new(),
    };
    if spec == NsxSpec::V21 {
        nsx.label = head.text_at(8, 16).ok_or_else(|| short("the label"))?;
        nsx.period = head.u32_at(24).ok_or_else(|| short("the period"))?;
        let n = head.u32_at(28).ok_or_else(|| short("the channel count"))?;
        if n == 0 || n > MAX_CHANNELS {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                28,
                format!("channel count {n}"),
            ));
        }
        nsx.header_len = SPEC21_HEADER_LEN + 4 * u64::from(n);
        let (src, factors) = match nev_factors {
            Some((src, m)) => (Some(src), m),
            None => (None, BTreeMap::new()),
        };
        for i in 0..n {
            let id = head
                .u32_at(SPEC21_HEADER_LEN + 4 * u64::from(i))
                .ok_or_else(|| short("the electrode ids"))?;
            let id = u16::try_from(id).unwrap_or(u16::MAX);
            let factor = factors.get(&id).map(|&r| spec21_factor(r));
            nsx.channels.push(NsxChannel {
                index: i,
                electrode_id: id,
                label: spec21_label(id),
                connector: None,
                pin: None,
                min_digital: None,
                max_digital: None,
                min_analog: None,
                max_analog: None,
                units: if factor.is_some() {
                    "uV".into()
                } else {
                    String::new()
                },
                highpass: None,
                lowpass: None,
                scale: factor.map_or(1.0, |x| x / 1000.0),
                offset: 0.0,
            });
        }
        if src.is_none() {
            findings.push(Finding::warning(
                "no_scaling",
                "spec 2.1 NSx files carry no analog range; no companion .nev with the same name was found, so values are raw counts",
            ));
        } else if nsx
            .channels
            .iter()
            .any(|c| !factors.contains_key(&c.electrode_id))
        {
            findings.push(Finding::warning(
                "partial_scaling",
                "the companion .nev lacks a waveform header for some electrodes; those channels are raw counts",
            ));
        }
        nsx.scaling_source = src;
        let data = file_len.saturating_sub(nsx.header_len);
        let frame = nsx.frame_len();
        if nsx.header_len > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                file_len,
                "file ends inside the header",
            ));
        }
        if !data.is_multiple_of(frame) {
            findings.push(Finding::warning(
                "partial_sample",
                format!(
                    "{} bytes after the last whole sample are ignored",
                    data % frame
                ),
            ));
        }
        nsx.packets.push(NsxPacket {
            offset: nsx.header_len,
            data_offset: nsx.header_len,
            timestamp: 0,
            points: data / frame,
        });
        nsx.sweeps.push(NsxSweep {
            first_packet: 0,
            packet_count: 1,
            sample_count: data / frame,
            timestamp: 0,
        });
        nsx.findings = findings;
        return Ok(nsx);
    }
    nsx.version = Some((head.u8_at(8).unwrap_or(0), head.u8_at(9).unwrap_or(0)));
    let bytes_in_headers = u64::from(head.u32_at(10).ok_or_else(|| short("the header length"))?);
    nsx.label = head.text_at(14, 16).unwrap_or_default();
    nsx.comment = head.text_at(30, 256).unwrap_or_default();
    nsx.period = head.u32_at(286).ok_or_else(|| short("the period"))?;
    nsx.time_resolution = head
        .u32_at(290)
        .ok_or_else(|| short("the timestamp resolution"))?;
    nsx.recorded_at = systemtime(&head, 294);
    let n = head.u32_at(310).ok_or_else(|| short("the channel count"))?;
    if n == 0 || n > MAX_CHANNELS {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            310,
            format!("channel count {n}"),
        ));
    }
    let expect = BASIC_HEADER_LEN + CC_LEN * u64::from(n);
    if bytes_in_headers != expect {
        findings.push(
            Finding::warning(
                "header_length_mismatch",
                format!("header says {bytes_in_headers} bytes; 314 + 66 × {n} channels = {expect}"),
            )
            .at(10),
        );
    }
    nsx.header_len = bytes_in_headers;
    let ext = if expect > head.end() {
        read_block(f, path, 0, expect, file_len)?
    } else {
        head
    };
    for i in 0..n {
        let at = BASIC_HEADER_LEN + CC_LEN * u64::from(i);
        if ext.slice(at, CC_LEN as usize).is_none() {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                "file ends inside the channel headers",
            ));
        }
        if ext.slice(at, 2) != Some(b"CC") {
            findings.push(
                Finding::warning(
                    "bad_channel_header",
                    format!("channel header {i} does not start with `CC`"),
                )
                .at(at),
            );
        }
        let (min_d, max_d, min_a, max_a) = (
            ext.i16_at(at + 22).unwrap_or(0),
            ext.i16_at(at + 24).unwrap_or(0),
            ext.i16_at(at + 26).unwrap_or(0),
            ext.i16_at(at + 28).unwrap_or(0),
        );
        let (scale, offset) = cc_scaling(min_d, max_d, min_a, max_a).unwrap_or_else(|| {
            findings.push(Finding::warning(
                "bad_scaling",
                format!("channel {i}: min and max digital value are equal; values are raw counts"),
            ));
            (1.0, 0.0)
        });
        nsx.channels.push(NsxChannel {
            index: i,
            electrode_id: ext.u16_at(at + 2).unwrap_or(0),
            label: ext.text_at(at + 4, 16).unwrap_or_default(),
            connector: ext.u8_at(at + 20),
            pin: ext.u8_at(at + 21),
            min_digital: Some(min_d),
            max_digital: Some(max_d),
            min_analog: Some(min_a),
            max_analog: Some(max_a),
            units: ext.text_at(at + 30, 16).unwrap_or_default(),
            highpass: Some((
                ext.u32_at(at + 46).unwrap_or(0),
                ext.u32_at(at + 50).unwrap_or(0),
                ext.u16_at(at + 54).unwrap_or(0),
            )),
            lowpass: Some((
                ext.u32_at(at + 56).unwrap_or(0),
                ext.u32_at(at + 60).unwrap_or(0),
                ext.u16_at(at + 64).unwrap_or(0),
            )),
            scale,
            offset,
        });
    }
    index_packets(f, path, &mut nsx, &mut findings, full_scan)?;
    nsx.findings = findings;
    Ok(nsx)
}

fn packet_at(b: &Block, at: u64, spec: NsxSpec) -> Option<(u8, u64, u64)> {
    let flag = b.u8_at(at)?;
    let (ts, n) = match spec {
        NsxSpec::V30 => (b.u64_at(at + 1)?, b.u32_at(at + 9)?),
        _ => (u64::from(b.u32_at(at + 1)?), b.u32_at(at + 5)?),
    };
    Some((flag, ts, u64::from(n)))
}

fn index_packets(
    f: &mut SourceFile,
    path: &Path,
    nsx: &mut NsxFile,
    findings: &mut Vec<Finding>,
    full_scan: bool,
) -> Result<()> {
    let file_len = nsx.file_len;
    let hl = nsx.spec.packet_header_len();
    let frame = nsx.frame_len();
    let mut pos = nsx.header_len;
    if pos >= file_len {
        findings.push(Finding::warning(
            "no_data",
            "the file ends after its header",
        ));
        return Ok(());
    }
    let first = read_block(f, path, pos, hl, file_len)?;
    let Some((flag, ts0, n0)) = packet_at(&first, pos, nsx.spec) else {
        findings.push(
            Finding::error(
                "truncated",
                "the file ends inside the first data-packet header",
            )
            .at(pos),
        );
        return Ok(());
    };
    if flag != 1 {
        findings.push(
            Finding::error(
                "bad_packet",
                format!("first data packet starts with {flag:#04x}, not 0x01"),
            )
            .at(pos),
        );
        return Ok(());
    }
    // PTP: one sample per packet at nanosecond resolution
    if nsx.time_resolution == PTP_RESOLUTION && n0 == 1 && nsx.spec == NsxSpec::V30 {
        let stride = hl + frame;
        let count = (file_len - pos) / stride;
        if !(file_len - pos).is_multiple_of(stride) {
            findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{} bytes after the last whole one-sample packet",
                        (file_len - pos) % stride
                    ),
                )
                .at(pos + count * stride),
            );
        }
        if count == 0 {
            // Not even the first one-sample packet is whole (reported just above).
            return Ok(());
        }
        nsx.ptp_stride = Some(stride);
        nsx.ptp_packet_count = count;
        nsx.packets.push(NsxPacket {
            offset: pos,
            data_offset: pos + hl,
            timestamp: ts0,
            points: 1,
        });
        let period_ns = f64::from(nsx.period) / PERIOD_CLOCK_HZ * 1e9;
        let last_at = pos + (count - 1) * stride;
        let last = read_block(f, path, last_at, hl, file_len)?;
        let ts_last = packet_at(&last, last_at, nsx.spec).map_or(ts0, |p| p.1);
        let expect = ts0 as f64 + (count - 1) as f64 * period_ns;
        if !full_scan && (ts_last as f64 - expect).abs() <= period_ns / 2.0 {
            nsx.sweeps.push(NsxSweep {
                first_packet: 0,
                packet_count: count,
                sample_count: count,
                timestamp: ts0,
            });
            return Ok(());
        }
        let mut prev: Option<u64> = None;
        let mut done = 0u64;
        while done < count {
            let batch = 65_536u64.min(count - done);
            let at = pos + done * stride;
            let chunk = read_block(f, path, at, batch * stride, file_len)?;
            for j in 0..batch {
                let ts = packet_at(&chunk, at + j * stride, nsx.spec).map_or(0, |p| p.1);
                let gap = prev
                    .is_none_or(|p| ((ts as f64 - p as f64) - period_ns).abs() > period_ns / 2.0);
                if gap {
                    nsx.sweeps.push(NsxSweep {
                        first_packet: done + j,
                        packet_count: 0,
                        sample_count: 0,
                        timestamp: ts,
                    });
                }
                let sweep = nsx.sweeps.last_mut().expect("pushed above");
                sweep.packet_count += 1;
                sweep.sample_count += 1;
                prev = Some(ts);
            }
            done += batch;
        }
        return Ok(());
    }
    loop {
        let hb = read_block(f, path, pos, hl, file_len)?;
        let Some((flag, ts, n)) = packet_at(&hb, pos, nsx.spec) else {
            findings.push(
                Finding::error("truncated", "the file ends inside a data-packet header").at(pos),
            );
            break;
        };
        if flag != 1 {
            findings.push(
                Finding::error(
                    "bad_packet",
                    format!("data packet at byte {pos} starts with {flag:#04x}, not 0x01"),
                )
                .at(pos),
            );
            break;
        }
        let data = pos + hl;
        let need = n.saturating_mul(frame);
        let avail = file_len.saturating_sub(data);
        let points = if need > avail {
            findings.push(Finding::error(
                "truncated",
                format!("data packet at byte {pos} declares {n} samples but the file holds {} ({:.1}% missing)", avail / frame, 100.0 * (n - avail / frame) as f64 / n.max(1) as f64),
            ).at(file_len));
            avail / frame
        } else {
            n
        };
        nsx.packets.push(NsxPacket {
            offset: pos,
            data_offset: data,
            timestamp: ts,
            points,
        });
        nsx.sweeps.push(NsxSweep {
            first_packet: nsx.packets.len() as u64 - 1,
            packet_count: 1,
            sample_count: points,
            timestamp: ts,
        });
        if need > avail {
            break;
        }
        pos = data + need;
        if pos >= file_len {
            break;
        }
        if nsx.packets.len() > 10_000_000 {
            findings.push(Finding::error(
                "bad_packet",
                "more than 10 million data packets",
            ));
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_and_labels() {
        let (s, o) = cc_scaling(-32764, 32764, -8191, 8191).unwrap();
        assert!((s - 0.25).abs() < 1e-3 && o.abs() < 1e-9);
        assert_eq!(cc_scaling(5, 5, 0, 1), None);
        assert_eq!(spec21_label(12), "chan12");
        assert_eq!(spec21_label(129), "ainp1");
        assert!((spec21_factor(21516) - 152_592.547).abs() < 1e-9);
    }

    #[test]
    fn systemtime_parse() {
        let mut b = Vec::new();
        for v in [2018u16, 3, 4, 1, 12, 36, 16, 678] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let blk = Block {
            origin: 0,
            bytes: b,
        };
        assert_eq!(
            systemtime(&blk, 0).as_deref(),
            Some("2018-03-01T12:36:16.678Z")
        );
    }
}
