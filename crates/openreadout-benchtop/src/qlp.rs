//! Bio-Rad QuantaSoft plate files (`.qlp`, `docs/formats/biorad-ddpcr.md` § QuantaSoft
//! `.qlp`): a TIFF-like file (`IILQ`, then a chain of IFDs of private tags). One IFD holds the
//! plate setup (a record per well: name, sample, experiment, the target of each channel), and
//! one IFD per read well holds its droplets: a 28-byte record per droplet (a counter, then per
//! channel its amplitude, a width and a third value), and one class byte per droplet.
//!
//! Table 0 `wells` has one row per read well and used channel, with the droplet counts by
//! class; table 1 `droplets` has one row per droplet of every read well.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::bytes::{le_f32, le_u16, le_u32, until_nul, latin1};
use openreadout_core::experiment::{
    Experiment, ExperimentInstrument, Measurement, MeasurementKind, Origin,
};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, FormatDescriptor, LsEntry, Table, TableInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

pub(crate) const FORMAT_ID: &str = "biorad-qlp";

/// Most IFDs followed (a 96-well plate has about 200).
const MAX_IFDS: usize = 20_000;
/// Most entries in one IFD.
const MAX_ENTRIES: u16 = 1024;
/// Bytes of one droplet record.
const DROPLET_BYTES: u64 = 28;
/// Bytes of one well-setup record (QuantaSoft 1.7).
const SETUP_BYTES: u64 = 1386;
/// Most droplets accepted in one well (a well holds about 20,000).
const MAX_DROPLETS: u32 = 2_000_000;

// tags, named in `docs/formats/biorad-ddpcr.md`
const TAG_SOFTWARE: u16 = 305;
const TAG_DATETIME: u16 = 306;
const TAG_USER: u16 = 315;
const TAG_HOST: u16 = 316;
const TAG_CHANNELS: u16 = 65004;
const TAG_SETUP: u16 = 65018;
const TAG_WELL: u16 = 65019;
const TAG_DROPLETS: u16 = 65021;
const TAG_CLASSES: u16 = 65057;
const TAG_DROPLET_VOLUME: u16 = 65078;
const TAG_SUPERMIX: u16 = 65077;

/// Is this the head of a `.qlp`?
pub(crate) fn sniff(head: &[u8]) -> bool {
    head.starts_with(b"IILQ")
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    /// The 4-byte value field: inline bytes or an offset.
    value: [u8; 4],
}

impl Entry {
    fn offset(&self) -> u64 {
        u64::from(u32::from_le_bytes(self.value))
    }
}

/// One channel's setup in a well.
#[derive(Debug, Clone, Default)]
struct ChannelSetup {
    /// `Unknown`, `Reference`, `NTC`, ... or `Not Used`.
    kind: String,
    target: String,
}

#[derive(Debug, Clone, Default)]
struct WellSetup {
    sample: String,
    experiment_type: String,
    experiment: String,
    channels: [ChannelSetup; 2],
}

/// A read well: where its droplets are.
#[derive(Debug, Clone)]
struct ReadWell {
    name: String,
    droplets: u32,
    records_at: u64,
    classes_at: u64,
    droplet_volume_nl: Option<f64>,
}

/// An opened `.qlp`.
#[derive(Debug)]
pub struct QlpDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    file: SourceFile,
    size: u64,
    software: Option<String>,
    written_at: Option<String>,
    user: Option<String>,
    host: Option<String>,
    channel_names: Vec<String>,
    setup: BTreeMap<String, WellSetup>,
    /// The supermix of each well, from its setup IFD.
    supermix: BTreeMap<String, String>,
    wells: Vec<ReadWell>,
    findings: Vec<Finding>,
    notes: Vec<String>,
    /// Rows of table 0: (well index, channel 0/1).
    rows: Vec<(usize, usize)>,
}

fn read_at(file: &SourceFile, size: u64, offset: u64, len: u64, path: &std::path::Path) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(len)
        .filter(|e| *e <= size)
        .ok_or_else(|| {
            Error::corrupt_at(
                FORMAT_ID,
                offset,
                format!("{len} bytes at offset {offset} run past the end of the {size}-byte file"),
            )
        })?;
    let n = usize::try_from(end - offset).map_err(|_| Error::corrupt(FORMAT_ID, "block too large"))?;
    let mut buf = vec![0u8; n];
    file.read_exact_at(offset, &mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// Text of a NUL-terminated field.
fn text(b: &[u8]) -> String {
    let s = until_nul(b);
    match std::str::from_utf8(s) {
        Ok(t) => t.trim().to_string(),
        Err(_) => latin1(s).trim().to_string(),
    }
}

impl QlpDataset {
    pub(crate) fn open(input: &Input, descriptor: FormatDescriptor) -> Result<Self> {
        let file = input.open()?;
        let path = input.path().to_path_buf();
        let size = file.size().map_err(|e| Error::io(&path, e))?;
        let head = read_at(&file, size, 0, 8, &path)?;
        if !sniff(&head) {
            return Err(Error::corrupt(FORMAT_ID, "no `IILQ` signature at byte 0"));
        }
        let mut ds = QlpDataset {
            descriptor,
            path,
            file,
            size,
            software: None,
            written_at: None,
            user: None,
            host: None,
            channel_names: Vec::new(),
            setup: BTreeMap::new(),
            supermix: BTreeMap::new(),
            wells: Vec::new(),
            findings: Vec::new(),
            notes: Vec::new(),
            rows: Vec::new(),
        };
        let mut next = u64::from(le_u32(&head, 4).unwrap_or(0));
        let mut seen = std::collections::BTreeSet::new();
        let mut first = true;
        while next != 0 {
            if !seen.insert(next) || seen.len() > MAX_IFDS {
                return Err(Error::corrupt_at(FORMAT_ID, next, "the IFD chain loops or has no end"));
            }
            let nb = read_at(&ds.file, size, next, 2, &ds.path)?;
            let n = le_u16(&nb, 0).unwrap_or(0);
            if n > MAX_ENTRIES {
                return Err(Error::corrupt_at(FORMAT_ID, next, format!("an IFD of {n} entries")));
            }
            let body = read_at(&ds.file, size, next + 2, u64::from(n) * 12 + 4, &ds.path)?;
            let entries: Vec<Entry> = (0..usize::from(n))
                .map(|i| {
                    let b = &body[i * 12..i * 12 + 12];
                    Entry {
                        tag: le_u16(b, 0).unwrap_or(0),
                        kind: le_u16(b, 2).unwrap_or(0),
                        count: le_u32(b, 4).unwrap_or(0),
                        value: [b[8], b[9], b[10], b[11]],
                    }
                })
                .collect();
            if first {
                ds.read_header(&entries)?;
                first = false;
            } else {
                ds.read_ifd(&entries)?;
            }
            next = u64::from(le_u32(&body, usize::from(n) * 12).unwrap_or(0));
        }
        if ds.wells.is_empty() {
            ds.notes.push("no well holds droplets (the plate was set up but not read)".into());
        }
        for (wi, w) in ds.wells.iter().enumerate() {
            let setup = ds.setup.get(&w.name);
            for ch in 0..2 {
                let used = setup.map_or(ch == 0, |s| {
                    let k = s.channels[ch].kind.to_ascii_lowercase();
                    !k.is_empty() && k != "not used"
                });
                if used {
                    ds.rows.push((wi, ch));
                }
            }
        }
        Ok(ds)
    }

    /// A string tag: up to 4 bytes inline, longer at its offset.
    fn string(&self, e: &Entry) -> Result<String> {
        if e.count <= 1 {
            return Ok(String::new());
        }
        if e.count <= 4 {
            return Ok(text(&e.value));
        }
        let b = read_at(&self.file, self.size, e.offset(), u64::from(e.count), &self.path)?;
        Ok(text(&b))
    }

    fn read_header(&mut self, entries: &[Entry]) -> Result<()> {
        for e in entries {
            let v = || self.string(e);
            match e.tag {
                TAG_SOFTWARE => self.software = Some(v()?).filter(|s| !s.is_empty()),
                TAG_DATETIME => self.written_at = Some(v()?).filter(|s| !s.is_empty()),
                TAG_USER => self.user = Some(v()?).filter(|s| !s.is_empty()),
                TAG_HOST => self.host = Some(v()?).filter(|s| !s.is_empty()),
                TAG_CHANNELS => {
                    self.channel_names = v()?
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_ifd(&mut self, entries: &[Entry]) -> Result<()> {
        let get = |t: u16| entries.iter().find(|e| e.tag == t);
        if let Some(e) = get(TAG_SETUP) {
            self.read_setup(e)?;
            return Ok(());
        }
        let Some(d) = get(TAG_DROPLETS) else {
            // a well's setup IFD: its supermix
            if let (Some(w), Some(m)) = (get(TAG_WELL), get(TAG_SUPERMIX)) {
                let m = self.string(m)?;
                if !m.is_empty() {
                    self.supermix.insert(text(&w.value), m);
                }
            }
            return Ok(());
        };
        if d.count == 0 {
            return Ok(());
        }
        let name = get(TAG_WELL).map(|e| text(&e.value)).unwrap_or_default();
        if d.count > MAX_DROPLETS {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("well {name}: {} droplets", d.count),
            ));
        }
        let records_end = u64::from(d.count)
            .checked_mul(DROPLET_BYTES)
            .and_then(|n| n.checked_add(d.offset()));
        if records_end.is_none_or(|e| e > self.size) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                d.offset(),
                format!("well {name}: the droplet records run past the end of the file"),
            ));
        }
        let Some(c) = get(TAG_CLASSES).filter(|c| c.count == d.count) else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("well {name} without one class byte per droplet"),
                "This QuantaSoft file stores droplets in a way the reader does not know; export the amplitudes as CSV from QuantaSoft.",
            ));
        };
        if c.offset().checked_add(u64::from(c.count)).is_none_or(|e| e > self.size) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                c.offset(),
                format!("well {name}: the droplet classes run past the end of the file"),
            ));
        }
        let volume = get(TAG_DROPLET_VOLUME)
            .filter(|e| e.kind == 11)
            .map(|e| f64::from(f32::from_le_bytes(e.value)))
            .filter(|v| v.is_finite() && *v > 0.0);
        self.wells.push(ReadWell {
            name,
            droplets: d.count,
            records_at: d.offset(),
            classes_at: c.offset(),
            droplet_volume_nl: volume,
        });
        Ok(())
    }

    /// The setup records: `count` records of 1386 bytes, each starting with the offset of the
    /// next one, so the layout checks itself.
    fn read_setup(&mut self, e: &Entry) -> Result<()> {
        let n = u64::from(e.count);
        let len = n.checked_mul(SETUP_BYTES).ok_or_else(|| Error::corrupt(FORMAT_ID, "setup size"))?;
        let b = read_at(&self.file, self.size, e.offset(), len, &self.path)?;
        for k in 0..n {
            let at = usize::try_from(k * SETUP_BYTES).unwrap_or(usize::MAX);
            let r = &b[at..at + SETUP_BYTES as usize];
            let next = u64::from(le_u32(r, 0).unwrap_or(0));
            // the last record points past the table (or nowhere)
            if k + 1 < n && next != e.offset() + (k + 1) * SETUP_BYTES {
                self.findings.push(Finding::warning(
                    "setup_not_decoded",
                    "the plate setup records do not have the layout this reader knows; samples and targets are left out",
                ));
                self.setup.clear();
                return Ok(());
            }
            let field = |a: usize, l: usize| text(&r[a..a + l]);
            let well = field(6, 4);
            if well.is_empty() {
                continue;
            }
            self.setup.insert(
                well,
                WellSetup {
                    sample: field(10, 256),
                    experiment_type: field(266, 32),
                    experiment: field(298, 512),
                    channels: [
                        ChannelSetup {
                            kind: field(810, 32),
                            target: field(842, 256),
                        },
                        ChannelSetup {
                            kind: field(1098, 32),
                            target: field(1130, 256),
                        },
                    ],
                },
            );
        }
        Ok(())
    }

    fn classes(&self, w: &ReadWell) -> Result<Vec<u8>> {
        read_at(&self.file, self.size, w.classes_at, u64::from(w.droplets), &self.path)
    }

    fn records(&self, w: &ReadWell) -> Result<Vec<u8>> {
        read_at(
            &self.file,
            self.size,
            w.records_at,
            u64::from(w.droplets) * DROPLET_BYTES,
            &self.path,
        )
    }

    fn droplet_rows(&self) -> u64 {
        self.wells.iter().map(|w| u64::from(w.droplets)).sum()
    }
}

/// A class byte as QuantaSoft's cluster number: 0 rejected, 1 both channels negative, 2 channel
/// 1 positive only, 3 both positive, 4 channel 2 positive only. Both halves of the byte carry the
/// number (`0x11`, `0x22`, ...); anything else is `None`.
fn cluster(b: u8) -> Option<u8> {
    let (hi, lo) = (b >> 4, b & 0x0F);
    (hi == lo && lo <= 4).then_some(lo)
}

fn positive(c: u8, ch: usize) -> bool {
    if ch == 0 {
        matches!(c, 2 | 3)
    } else {
        matches!(c, 3 | 4)
    }
}

const WELL_COLUMNS: &[(&str, &str, Option<&str>)] = &[
    ("well", "well", None),
    ("sample", "sample", None),
    ("experiment_type", "experiment type", None),
    ("experiment", "experiment", None),
    ("channel", "channel", None),
    ("target_type", "target type", None),
    ("target", "target", None),
    ("supermix", "supermix", None),
    ("droplets", "droplets stored", Some("droplets")),
    ("accepted_droplets", "accepted droplets (class 1 to 4)", Some("droplets")),
    ("positive_droplets", "positive droplets in this channel", Some("droplets")),
    ("negative_droplets", "negative droplets in this channel", Some("droplets")),
    ("droplets_ch1_pos_ch2_pos", "Ch1+Ch2+ droplets (class 3)", Some("droplets")),
    ("droplets_ch1_pos_ch2_neg", "Ch1+Ch2- droplets (class 2)", Some("droplets")),
    ("droplets_ch1_neg_ch2_pos", "Ch1-Ch2+ droplets (class 4)", Some("droplets")),
    ("droplets_ch1_neg_ch2_neg", "Ch1-Ch2- droplets (class 1)", Some("droplets")),
    ("droplet_volume", "droplet volume", Some("nL")),
];

const TEXT_COLUMNS: usize = 8;

impl QlpDataset {
    /// Per row of table 0, the text cells.
    fn well_text(&self, wi: usize, ch: usize) -> [String; TEXT_COLUMNS] {
        let w = &self.wells[wi];
        let s = self.setup.get(&w.name);
        let chan = self
            .channel_names
            .get(ch)
            .cloned()
            .unwrap_or_else(|| format!("Ch{}", ch + 1));
        [
            w.name.clone(),
            s.map(|s| s.sample.clone()).unwrap_or_default(),
            s.map(|s| s.experiment_type.clone()).unwrap_or_default(),
            s.map(|s| s.experiment.clone()).unwrap_or_default(),
            chan,
            s.map(|s| s.channels[ch].kind.clone()).unwrap_or_default(),
            s.map(|s| s.channels[ch].target.clone()).unwrap_or_default(),
            self.supermix.get(&w.name).cloned().unwrap_or_default(),
        ]
    }

    /// Category labels of each text column of table 0, in order of first appearance.
    fn text_categories(&self) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = vec![Vec::new(); TEXT_COLUMNS];
        for &(wi, ch) in &self.rows {
            for (k, v) in self.well_text(wi, ch).into_iter().enumerate() {
                if !out[k].contains(&v) {
                    out[k].push(v);
                }
            }
        }
        out
    }

    /// The counts of one well: (stored, accepted, per class 1..4), or `None` when the classes
    /// look unset (no droplet rejected and every droplet in one class).
    fn counts(&self, w: &ReadWell) -> Result<Option<(u64, [u64; 5])>> {
        let cl = self.classes(w)?;
        let mut by = [0u64; 5];
        for b in &cl {
            match cluster(*b) {
                Some(c) => by[usize::from(c)] += 1,
                None => return Ok(None),
            }
        }
        if by[0] == 0 && by[1..].iter().filter(|n| **n > 0).count() <= 1 {
            return Ok(None);
        }
        Ok(Some((cl.len() as u64, by)))
    }
}

fn column(index: usize, name: &str, label: &str, unit: Option<&str>, cats: Option<&[String]>) -> ColumnInfo {
    let mut extra = BTreeMap::new();
    if let Some(c) = cats {
        extra.insert("categories".to_string(), json!(c));
    }
    ColumnInfo {
        index: index as u32,
        name: name.into(),
        label: Some(label.into()),
        dtype: if cats.is_some() { "uint32" } else { "float64" }.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra,
    }
}

impl Dataset for QlpDataset {
    fn info(&self) -> Result<FileInfo> {
        let cats = self.text_categories();
        let wells = TableInfo {
            index: 0,
            name: Some("wells".into()),
            row_count: self.rows.len() as u64,
            columns: WELL_COLUMNS
                .iter()
                .enumerate()
                .map(|(k, (n, l, u))| column(k, n, l, *u, (k < TEXT_COLUMNS).then(|| cats[k].as_slice())))
                .collect(),
            extra: BTreeMap::from([("kind".to_string(), json!("well_channels"))]),
        };
        let well_names: Vec<String> = self.wells.iter().map(|w| w.name.clone()).collect();
        let droplets = TableInfo {
            index: 1,
            name: Some("droplets".into()),
            row_count: self.droplet_rows(),
            columns: vec![
                column(0, "well", "well", None, Some(&well_names)),
                column(1, "counter", "a counter that increases from droplet to droplet (unit not known)", None, None),
                column(2, "ch1_amplitude", "channel 1 amplitude", None, None),
                column(3, "ch2_amplitude", "channel 2 amplitude", None, None),
                column(4, "width", "droplet width (the reader's units)", None, None),
                column(5, "cluster", "cluster: 0 rejected, 1 Ch1-Ch2-, 2 Ch1+Ch2-, 3 Ch1+Ch2+, 4 Ch1-Ch2+ (NaN: not a known class)", None, None),
            ],
            extra: BTreeMap::from([("kind".to_string(), json!("droplets"))]),
        };
        let mut notes = self.notes.clone();
        notes.push(format!(
            "table 0 has one row per read well and used channel ({} wells read); table 1 has one row per droplet of those wells",
            self.wells.len()
        ));
        notes.push("droplet counts are tallied from the stored droplet classes; QuantaSoft's concentrations are not stored and not computed".into());
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: self.software.clone(),
            plane_count: 0,
            images: Vec::new(),
            tables: vec![wells, droplets],
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "software": self.software,
            "written": self.written_at,
            "user": self.user,
            "host": self.host,
            "channels": self.channel_names,
            "wells_set_up": self.setup.len(),
            "wells_read": self.wells.iter().map(|w| json!({"well": w.name, "droplets": w.droplets})).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("tables".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a .qlp holds tables (0: wells, 1: droplets), not images".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("IFD chain walked; every read well's droplet records and classes lie inside the file");
        let mut unknown = 0usize;
        for w in &self.wells {
            unknown += self.classes(w)?.iter().filter(|b| cluster(**b).is_none()).count();
        }
        r.performed("every droplet class byte read");
        if unknown > 0 {
            r.push(Finding::warning(
                "unknown_droplet_class",
                format!("{unknown} droplet(s) have a class byte the reader does not know"),
            ));
        }
        for f in &self.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        match index {
            0 => {
                let cats = self.text_categories();
                let n = self.rows.len() as u64;
                let a = first_row.min(n) as usize;
                let b = first_row.saturating_add(max_rows).min(n) as usize;
                let mut cols: Vec<Vec<f64>> = vec![Vec::new(); WELL_COLUMNS.len()];
                let mut cache: BTreeMap<usize, Option<(u64, [u64; 5])>> = BTreeMap::new();
                for &(wi, ch) in &self.rows[a..b] {
                    for (k, v) in self.well_text(wi, ch).iter().enumerate() {
                        cols[k].push(cats[k].iter().position(|c| c == v).unwrap_or(0) as f64);
                    }
                    let w = &self.wells[wi];
                    if !cache.contains_key(&wi) {
                        cache.insert(wi, self.counts(w)?);
                    }
                    let nan = f64::NAN;
                    let (stored, by) = match cache[&wi] {
                        Some((s, by)) => (s as f64, Some(by)),
                        None => (f64::from(w.droplets), None),
                    };
                    let f = |x: u64| x as f64;
                    let (acc, pos, neg, c) = match by {
                        Some(by) => {
                            let acc = by[1..].iter().sum::<u64>();
                            let pos = (1..5u8).filter(|c| positive(*c, ch)).map(|c| by[usize::from(c)]).sum::<u64>();
                            (f(acc), f(pos), f(acc - pos), [f(by[3]), f(by[2]), f(by[4]), f(by[1])])
                        }
                        None => (nan, nan, nan, [nan; 4]),
                    };
                    let t = TEXT_COLUMNS;
                    cols[t].push(stored);
                    cols[t + 1].push(acc);
                    cols[t + 2].push(pos);
                    cols[t + 3].push(neg);
                    for (j, v) in c.into_iter().enumerate() {
                        cols[t + 4 + j].push(v);
                    }
                    cols[t + 8].push(w.droplet_volume_nl.unwrap_or(nan));
                }
                Ok(Table { table: 0, first_row: a as u64, columns: cols })
            }
            1 => {
                let n = self.droplet_rows();
                let a = first_row.min(n);
                let b = first_row.saturating_add(max_rows).min(n);
                let mut cols: Vec<Vec<f64>> = vec![Vec::new(); 6];
                let mut start = 0u64;
                for (wi, w) in self.wells.iter().enumerate() {
                    let end = start + u64::from(w.droplets);
                    if end > a && start < b {
                        let rec = self.records(w)?;
                        let cl = self.classes(w)?;
                        let lo = a.saturating_sub(start) as usize;
                        let hi = (b.min(end) - start) as usize;
                        for k in lo..hi {
                            let r = &rec[k * 28..k * 28 + 28];
                            cols[0].push(wi as f64);
                            cols[1].push(f64::from(le_u32(r, 0).unwrap_or(0)));
                            cols[2].push(f64::from(le_f32(r, 4).unwrap_or(f32::NAN)));
                            cols[3].push(f64::from(le_f32(r, 16).unwrap_or(f32::NAN)));
                            cols[4].push(f64::from(le_f32(r, 8).unwrap_or(f32::NAN)));
                            cols[5].push(cluster(cl[k]).map_or(f64::NAN, f64::from));
                        }
                    }
                    start = end;
                }
                Ok(Table { table: 1, first_row: a, columns: cols })
            }
            _ => Err(Error::Usage(format!(
                "table {index} does not exist (0: wells, 1: droplets)"
            ))),
        }
    }

    fn experiment(&self) -> Option<Experiment> {
        let mut exp = Experiment::default();
        let inferred = |from: &str| Origin {
            source: Source::Inferred,
            from: from.into(),
        };
        let (software, version) = match self.software.as_deref() {
            Some(s) => match s.rsplit_once(' ') {
                Some((a, b)) if b.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                    (Some(a.to_string()), Some(b.to_string()))
                }
                _ => (Some(s.to_string()), None),
            },
            None => (None, None),
        };
        exp.instrument = Some(ExperimentInstrument {
            vendor: Some("Bio-Rad".into()),
            model: None,
            serial: None,
            software,
            software_version: version,
            kind: None,
        });
        exp.provenance.insert("instrument.vendor".into(), inferred("the `.qlp` signature"));
        if self.software.is_some() {
            exp.provenance.insert("instrument.software".into(), inferred("TIFF tag 305 of the first IFD"));
        }
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Table,
            indices: vec![0, 1],
            what: format!("{} well(s) read, {} droplet(s)", self.wells.len(), self.droplet_rows()),
            technique: None,
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        exp.provenance.insert("measurements[0]".into(), inferred("the reader's data layout"));
        Some(exp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clusters() {
        assert_eq!(cluster(0x00), Some(0));
        assert_eq!(cluster(0x11), Some(1));
        assert_eq!(cluster(0x33), Some(3));
        assert_eq!(cluster(0x44), Some(4));
        assert_eq!(cluster(0x12), None);
        assert_eq!(cluster(0x55), None);
        assert!(positive(3, 0) && positive(3, 1) && positive(2, 0) && !positive(2, 1));
    }
}
