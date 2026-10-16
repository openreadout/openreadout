//! POD5 (`.pod5`): Arrow IPC tables (reads, signal, run info) bundled in a container with a
//! FlatBuffer footer (`docs/formats/nanopore.md`).
//!
//! `open` reads the footer, the reads table and the run-info table (small: one row per read
//! and per run) and the footer of the signal table (where its record batches are). A read's
//! signal is decoded on demand from the signal rows the reads table lists.

use std::collections::HashMap;

use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, Result};

use super::ipc::{self, BatchBlock, ColType, Column, IpcFile};
use super::{ReadMeta, RunMeta, SignalSource, vbz};

/// Format id of POD5 files.
pub const FORMAT_ID: &str = "pod5";

/// The POD5 signature at both ends of the file.
pub const SIGNATURE: &[u8; 8] = b"\x8BPOD\r\n\x1A\n";

/// Largest POD5 footer read.
const MAX_FOOTER: u64 = 16 << 20;

/// True when `head` starts with the POD5 signature.
pub fn looks_like(head: &[u8]) -> bool {
    head.starts_with(SIGNATURE)
}

/// An embedded file listed in the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Embedded {
    /// Offset in the POD5 file.
    pub offset: u64,
    /// Length.
    pub length: u64,
    /// 0 reads table, 1 signal table, 2 read-id index, 3 other index.
    pub content: i16,
}

/// What the footer says.
#[derive(Debug, Clone, Default)]
pub struct Footer {
    /// `MINKNOW:file_identifier`.
    pub file_identifier: String,
    /// Software that wrote the file.
    pub software: String,
    /// POD5 specification version of the tables.
    pub pod5_version: String,
    /// Embedded files.
    pub contents: Vec<Embedded>,
}

/// Bounds-checked FlatBuffer table access (the footer schema in `docs/formats/nanopore.md`).
struct Fb<'a> {
    b: &'a [u8],
}

impl<'a> Fb<'a> {
    fn u32_at(&self, at: usize) -> Option<u32> {
        let s = self.b.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32_at(&self, at: usize) -> Option<i32> {
        self.u32_at(at).map(|v| v as i32)
    }
    fn u16_at(&self, at: usize) -> Option<u16> {
        let s = self.b.get(at..at.checked_add(2)?)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn i64_at(&self, at: usize) -> Option<i64> {
        let s = self.b.get(at..at.checked_add(8)?)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(i64::from_le_bytes(a))
    }
    /// Position of field `i` of the table at `t`, when present.
    fn field(&self, t: usize, i: usize) -> Option<usize> {
        let vt = (t as i64).checked_sub(i64::from(self.i32_at(t)?))?;
        let vt = usize::try_from(vt).ok()?;
        let vlen = self.u16_at(vt)? as usize;
        let slot = 4 + 2 * i;
        if slot + 2 > vlen {
            return None;
        }
        let off = self.u16_at(vt + slot)? as usize;
        (off != 0).then(|| t + off)
    }
    fn deref(&self, at: usize) -> Option<usize> {
        at.checked_add(self.u32_at(at)? as usize)
    }
    fn string(&self, t: usize, i: usize) -> Option<String> {
        let p = self.deref(self.field(t, i)?)?;
        let n = self.u32_at(p)? as usize;
        let s = self.b.get(p + 4..(p + 4).checked_add(n)?)?;
        Some(String::from_utf8_lossy(s).into_owned())
    }
}

/// Parse the footer FlatBuffer.
pub fn parse_footer(b: &[u8]) -> std::result::Result<Footer, String> {
    let fb = Fb { b };
    let root = fb.deref(0).ok_or("footer too short")?;
    let mut f = Footer {
        file_identifier: fb.string(root, 0).unwrap_or_default(),
        software: fb.string(root, 1).unwrap_or_default(),
        pod5_version: fb.string(root, 2).unwrap_or_default(),
        contents: Vec::new(),
    };
    let v = fb
        .field(root, 3)
        .and_then(|p| fb.deref(p))
        .ok_or("footer lists no embedded tables")?;
    let n = fb.u32_at(v).ok_or("footer table list truncated")? as usize;
    if n > 1024 {
        return Err(format!("footer lists {n} embedded tables"));
    }
    for k in 0..n {
        let at = v + 4 + 4 * k;
        let t = fb.deref(at).ok_or("footer table entry truncated")?;
        let get64 = |i| fb.field(t, i).and_then(|p| fb.i64_at(p)).unwrap_or(0);
        let content = fb
            .field(t, 3)
            .and_then(|p| fb.u16_at(p))
            .map_or(0, |v| v as i16);
        let (o, l) = (get64(0), get64(1));
        if o < 0 || l < 0 {
            return Err("negative embedded table offset".into());
        }
        f.contents.push(Embedded {
            offset: o as u64,
            length: l as u64,
            content,
        });
    }
    Ok(f)
}

fn corrupt(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, detail)
}

fn read_range(f: &SourceFile, offset: u64, len: u64, what: &str) -> Result<Vec<u8>> {
    if len > ipc::MAX_TABLE_BYTES {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("{what} of {len} bytes"),
            "This POD5 table is larger than the reader's limit; split the file with the pod5 tools (pod5 subset).",
        ));
    }
    let mut b = vec![0u8; len as usize];
    f.read_exact_at(offset, &mut b)
        .map_err(|e| corrupt(format!("{what}: {e} (the file is truncated)")))?;
    Ok(b)
}

/// The signal table: its location and record batches, read batch by batch.
#[derive(Debug)]
pub struct Pod5Signal {
    file: SourceFile,
    table_offset: u64,
    ipc: IpcFile,
    /// First signal row of each batch.
    starts: Vec<u64>,
    signal_col: usize,
    samples_col: usize,
    vbz: bool,
    cache: Option<(usize, Vec<Option<Vec<u8>>>, Vec<Option<i64>>)>,
    /// Signal rows of each read.
    rows: Vec<Vec<u64>>,
}

impl Pod5Signal {
    fn batch(&mut self, bi: usize) -> Result<()> {
        if self.cache.as_ref().is_some_and(|c| c.0 == bi) {
            return Ok(());
        }
        let b = self.ipc.batches[bi];
        let bytes = read_range(
            &self.file,
            self.table_offset + b.offset,
            b.meta_len + b.body_len,
            "signal record batch",
        )?;
        let local = BatchBlock { offset: 0, ..b };
        let (meta, body) = ipc::block_bytes(&bytes, &local).map_err(corrupt)?;
        let m = ipc::parse_message(meta).map_err(corrupt)?;
        let dicts = HashMap::new();
        let mut r = ipc::BatchReader::new(body, &m, &dicts);
        let mut sig = None;
        let mut samples = None;
        for (k, f) in self.ipc.fields.iter().enumerate() {
            let c = r.column(&f.ty).map_err(corrupt)?;
            if k == self.signal_col {
                sig = Some(c);
            } else if k == self.samples_col {
                samples = Some(c);
            }
        }
        let sig = match sig {
            Some(Column::Bytes(v)) => v,
            Some(Column::IntList(v)) => v
                .into_iter()
                .map(|l| {
                    l.map(|l| {
                        l.into_iter()
                            .flat_map(|x| (x as i16).to_le_bytes())
                            .collect()
                    })
                })
                .collect(),
            _ => return Err(corrupt("signal table has no readable signal column")),
        };
        let samples = match samples {
            Some(Column::Int(v)) => v,
            _ => vec![None; sig.len()],
        };
        self.cache = Some((bi, sig, samples));
        Ok(())
    }
}

impl SignalSource for Pod5Signal {
    fn raw(&mut self, read: usize) -> Result<Vec<i16>> {
        let rows = self
            .rows
            .get(read)
            .cloned()
            .ok_or_else(|| Error::Usage(format!("read {read} does not exist")))?;
        let mut out = Vec::new();
        for row in rows {
            let bi = self.starts.partition_point(|s| *s <= row).saturating_sub(1);
            if bi >= self.ipc.batches.len() {
                return Err(corrupt(format!("signal row {row} is past the signal table")));
            }
            self.batch(bi)?;
            let Some((_, sig, samples)) = &self.cache else {
                return Err(corrupt("signal batch"));
            };
            let k = usize::try_from(row - self.starts[bi]).unwrap_or(usize::MAX);
            let bytes = sig
                .get(k)
                .and_then(Option::as_ref)
                .ok_or_else(|| corrupt(format!("signal row {row} is missing")))?;
            if self.vbz {
                let n = samples
                    .get(k)
                    .copied()
                    .flatten()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| corrupt(format!("signal row {row} has no sample count")))?;
                let v = vbz::pod5_row(bytes, n)
                    .map_err(|e| corrupt(format!("signal row {row}: {e}")))?;
                out.extend(v);
            } else {
                out.extend(
                    bytes
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]])),
                );
            }
            if out.len() > vbz::MAX_SAMPLES {
                return Err(corrupt("a read of more than 2^30 samples"));
            }
        }
        Ok(out)
    }
}

/// An open POD5 file: footer, reads, runs and the signal table.
#[derive(Debug)]
pub struct Pod5 {
    /// What the footer says.
    pub footer: Footer,
    /// Reads, in reads-table order.
    pub reads: Vec<ReadMeta>,
    /// Runs (acquisitions).
    pub runs: Vec<RunMeta>,
    /// The signal table.
    pub signal: Pod5Signal,
    /// Schema metadata of the reads table.
    pub table_metadata: Vec<(String, String)>,
    /// File size.
    pub size: u64,
}

impl Pod5 {
    /// True when the signal is VBZ-compressed (else stored as plain 16-bit lists).
    pub fn signal_is_vbz(&self) -> bool {
        self.signal.vbz
    }
}

fn col<'a>(cols: &'a [Column], ipc: &IpcFile, name: &str) -> Option<&'a Column> {
    ipc::field_index(ipc, name).and_then(|i| cols.get(i))
}

fn ints(c: Option<&Column>, i: usize) -> Option<i64> {
    match c? {
        Column::Int(v) => v.get(i).copied().flatten(),
        _ => None,
    }
}

fn floats(c: Option<&Column>, i: usize) -> Option<f64> {
    match c? {
        Column::Float(v) => v.get(i).copied().flatten().filter(|x| x.is_finite()),
        Column::Int(v) => v.get(i).copied().flatten().map(|x| x as f64),
        _ => None,
    }
}

fn text(c: Option<&Column>, i: usize) -> Option<String> {
    match c? {
        Column::Text(v) => v.get(i).cloned().flatten(),
        Column::Bytes(v) => v
            .get(i)
            .cloned()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// A 16-byte UUID as text.
pub fn uuid(b: &[u8]) -> String {
    if b.len() != 16 {
        return b.iter().map(|x| format!("{x:02x}")).collect();
    }
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

/// Open a POD5 file.
pub fn open(input: &Input) -> Result<Pod5> {
    let path = input.path();
    let file = input.open()?;
    let size = file.size().map_err(|e| Error::io(path, e))?;
    if size < 64 {
        return Err(corrupt("shorter than a POD5 header and footer"));
    }
    let mut tail = [0u8; 32];
    file.read_exact_at(size - 32, &mut tail)
        .map_err(|e| Error::io(path, e))?;
    if &tail[24..32] != SIGNATURE {
        return Err(Error::Corrupt {
            format: FORMAT_ID,
            detail: "no POD5 signature at the end of the file: it is truncated or still being written".into(),
            offset: Some(size),
        });
    }
    let flen = i64::from_le_bytes(tail[0..8].try_into().unwrap_or([0; 8]));
    if flen <= 0 || flen as u64 > MAX_FOOTER || flen as u64 > size - 32 {
        return Err(corrupt(format!("footer length {flen} is not possible")));
    }
    let fstart = size - 32 - flen as u64;
    let fb = read_range(&file, fstart, flen as u64, "footer")?;
    let footer = parse_footer(&fb).map_err(corrupt)?;
    let find = |c: i16| footer.contents.iter().find(|e| e.content == c).copied();
    let reads_e = find(0).ok_or_else(|| corrupt("footer lists no reads table"))?;
    let signal_e = find(1).ok_or_else(|| corrupt("footer lists no signal table"))?;
    for e in &footer.contents {
        if e.offset.saturating_add(e.length) > size {
            return Err(corrupt("an embedded table runs past the end of the file"));
        }
    }
    // reads table
    let rb = read_range(&file, reads_e.offset, reads_e.length, "reads table")?;
    let mut rt = ipc::parse_footer(&rb, 0, reads_e.length)
        .map_err(|e| corrupt(format!("reads table: {e}")))?;
    if ipc::field_index(&rt, "calibration_offset").is_none()
        && ipc::field_index(&rt, "calibration").is_some()
    {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!(
                "POD5 {} reads table (pore, calibration and end reason as dictionaries of structs)",
                footer.pod5_version
            ),
            "Files written by pod5 before 0.1 are not read; `pod5 update FILE` (from the pod5 tools) rewrites them in the current table layout.",
        ));
    }
    let rc = ipc::read_all(&rb, &mut rt).map_err(|e| corrupt(format!("reads table: {e}")))?;
    // run info: its own table (POD5 0.1 and later), else columns of the reads table
    let mut runs = Vec::new();
    let mut run_index: HashMap<String, usize> = HashMap::new();
    let run_table = footer
        .contents
        .iter()
        .filter(|e| e.content >= 3 || e.content < 0)
        .copied()
        .collect::<Vec<_>>();
    for e in run_table {
        let b = read_range(&file, e.offset, e.length, "run info table")?;
        let Ok(mut t) = ipc::parse_footer(&b, 0, e.length) else {
            continue;
        };
        if ipc::field_index(&t, "acquisition_id").is_none() {
            continue;
        }
        let cols = ipc::read_all(&b, &mut t).map_err(|e| corrupt(format!("run info table: {e}")))?;
        let n = t.batches.iter().map(|b| b.rows).sum::<u64>() as usize;
        for i in 0..n {
            let g = |name: &str| text(col(&cols, &t, name), i);
            let map = |name: &str| match col(&cols, &t, name) {
                Some(Column::Map(v)) => v.get(i).cloned().flatten().unwrap_or_default(),
                _ => Vec::new(),
            };
            let r = RunMeta {
                acquisition_id: g("acquisition_id").unwrap_or_default(),
                started_ms: ints(col(&cols, &t, "acquisition_start_time"), i),
                protocol_started_ms: ints(col(&cols, &t, "protocol_start_time"), i),
                experiment: g("experiment_name"),
                flow_cell_id: g("flow_cell_id"),
                flow_cell_product: g("flow_cell_product_code"),
                protocol: g("protocol_name"),
                protocol_run_id: g("protocol_run_id"),
                sample_id: g("sample_id"),
                sample_rate: floats(col(&cols, &t, "sample_rate"), i),
                sequencing_kit: g("sequencing_kit"),
                position: g("sequencer_position"),
                position_type: g("sequencer_position_type"),
                software: g("software"),
                system_name: g("system_name"),
                system_type: g("system_type"),
                adc_min: ints(col(&cols, &t, "adc_min"), i),
                adc_max: ints(col(&cols, &t, "adc_max"), i),
                context_tags: map("context_tags"),
                tracking_id: map("tracking_id"),
            };
            run_index.insert(r.acquisition_id.clone(), runs.len());
            runs.push(r);
        }
    }
    let n = rt.batches.iter().map(|b| b.rows).sum::<u64>() as usize;
    let mut reads = Vec::with_capacity(n);
    let mut rows = Vec::with_capacity(n);
    for i in 0..n {
        let id = match col(&rc, &rt, "read_id") {
            Some(Column::Bytes(v)) => v.get(i).cloned().flatten().map(|b| uuid(&b)),
            _ => None,
        }
        .unwrap_or_default();
        let sig_rows: Vec<u64> = match col(&rc, &rt, "signal") {
            Some(Column::IntList(v)) => v
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_default()
                .into_iter()
                .map(|x| x.max(0) as u64)
                .collect(),
            _ => Vec::new(),
        };
        let run_name = text(col(&rc, &rt, "run_info"), i);
        let run = run_name.as_ref().and_then(|r| run_index.get(r).copied());
        let channel = ints(col(&rc, &rt, "channel_32bit"), i)
            .or_else(|| ints(col(&rc, &rt, "channel"), i));
        let rate = run
            .and_then(|r| runs.get(r))
            .and_then(|r: &RunMeta| r.sample_rate);
        let num_samples = ints(col(&rc, &rt, "num_samples"), i).map(|x| x.max(0) as u64);
        reads.push(ReadMeta {
            read_id: id,
            channel: channel.map(|c| c as u32),
            well: ints(col(&rc, &rt, "well"), i).map(|c| c as u32),
            read_number: ints(col(&rc, &rt, "read_number"), i).map(|c| c.max(0) as u64),
            start_sample: ints(col(&rc, &rt, "start"), i).map(|c| c.max(0) as u64),
            num_samples,
            median_before: floats(col(&rc, &rt, "median_before"), i),
            open_pore_level: floats(col(&rc, &rt, "open_pore_level"), i),
            end_reason: text(col(&rc, &rt, "end_reason"), i),
            end_reason_forced: match col(&rc, &rt, "end_reason_forced") {
                Some(Column::Bool(v)) => v.get(i).copied().flatten(),
                _ => None,
            },
            pore_type: text(col(&rc, &rt, "pore_type"), i),
            num_events: ints(col(&rc, &rt, "num_minknow_events"), i).map(|x| x.max(0) as u64),
            offset: floats(col(&rc, &rt, "calibration_offset"), i).unwrap_or(0.0),
            scale: floats(col(&rc, &rt, "calibration_scale"), i).unwrap_or(1.0),
            run,
            run_name,
            sample_rate: rate,
        });
        rows.push(sig_rows);
    }
    // signal table: footer only
    let tail_len = signal_e.length.min(MAX_FOOTER + 16);
    let tail_at = signal_e.offset + signal_e.length - tail_len;
    let st_tail = read_range(&file, tail_at, tail_len, "signal table footer")?;
    let mut st = ipc::parse_footer(&st_tail, tail_at - signal_e.offset, signal_e.length)
        .map_err(|e| corrupt(format!("signal table: {e}")))?;
    let signal_col = ipc::field_index(&st, "signal")
        .ok_or_else(|| corrupt("signal table has no signal column"))?;
    let samples_col = ipc::field_index(&st, "samples").unwrap_or(usize::MAX);
    let vbz = st.fields[signal_col].extension.as_deref() == Some("minknow.vbz")
        || matches!(st.fields[signal_col].ty, ColType::Binary { .. });
    // row counts per batch, from each batch's message metadata
    let mut starts = Vec::with_capacity(st.batches.len());
    let mut acc = 0u64;
    for b in &mut st.batches {
        let meta = read_range(
            &file,
            signal_e.offset + b.offset,
            b.meta_len,
            "signal batch header",
        )?;
        let m = ipc::parse_message(&meta).map_err(|e| corrupt(format!("signal table: {e}")))?;
        b.rows = m.rows;
        starts.push(acc);
        acc = acc.saturating_add(m.rows);
    }
    let signal = Pod5Signal {
        file,
        table_offset: signal_e.offset,
        ipc: st,
        starts,
        signal_col,
        samples_col,
        vbz,
        cache: None,
        rows,
    };
    Ok(Pod5 {
        footer,
        reads,
        runs,
        signal,
        table_metadata: rt.metadata,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footers_that_are_not_flatbuffers_are_errors() {
        assert!(parse_footer(&[]).is_err());
        assert!(parse_footer(&[4, 0, 0, 0, 0, 0, 0, 0]).is_err());
        for n in 0..40u8 {
            let b: Vec<u8> = (0..n).map(|i| i.wrapping_mul(29)).collect();
            let _ = parse_footer(&b);
        }
        assert_eq!(
            uuid(&[0u8; 16]),
            "00000000-0000-0000-0000-000000000000"
        );
    }
}
