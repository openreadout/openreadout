//! `Dataset` for Thermo Scientific Chromeleon 7 archives (`.cmbx`): one trace per archived 2D
//! signal (every injection of the sequence, every channel), described by the sequence file and
//! decoded from its member on demand. Layout: `docs/formats/chromeleon.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    AttachmentInfo, CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table,
    TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::{filetime_to_iso8601, iso8601_to_unix};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::binary::tidy;
use crate::chromeleon::{
    AUDIT_TRAIL_ITEM, ArchiveHeader, DecodedSignal, FieldDescription, FieldIndex, INJECTION_ITEM,
    MAX_HEADER_BYTES, MAX_SEQUENCE_BYTES, MAX_SIGNAL_BYTES, MS_RAW_ITEM, PROCESSING_METHOD_ITEM,
    SEQUENCE_ITEM, SIGNAL_ITEM, SPECTRAL_FIELD_ITEM, SignalDescription, SignalError, SpectrumGrid,
    decode_signal, decode_spectrum, index_field, parse_header, parse_sequence_fields,
    parse_sequence_signals,
};
use crate::chromeleon_results::{
    SequenceContents, StoredPeak, StoredResults, audit_messages, blob_magic, blob_stated_size,
    integrate_stored_peak, method_steps, parse_sequence_contents, unpack_blob,
};
use crate::{CHROMELEON_ID, ChromeleonReader};
use openreadout_core::zip::ZipIndex;

/// One archived 2D signal (a trace).
#[derive(Debug, Clone)]
struct SignalEntry {
    /// Index into the header's items.
    item: usize,
    /// The enclosing injection item.
    injection: Option<usize>,
    /// Ordinal of the injection among the archive's injections.
    injection_index: Option<usize>,
    /// The enclosing sequence item.
    sequence: Option<usize>,
    /// The sequence file's description (matched by file id).
    desc: Option<SignalDescription>,
    /// Decoded at open when there is no description to take the grid from.
    decoded: Option<std::result::Result<DecodedSignal, String>>,
    /// The sequence file's details of its injection (index into `contents.injections`).
    details: Option<usize>,
    /// Its chromatogram with Chromeleon's stored results (index into `contents.chromatograms`).
    chrom: Option<usize>,
    /// For a spectral field (3D data): its description and the grid of its first spectrum.
    field: Option<FieldEntry>,
    /// The point encoding, from the member's first section after `SignHdr` (`PtsLDiff`,
    /// `PtsLL2Df`, `PtsDDCmp`; `3DRawSpc` for a spectral field).
    encoding: Option<String>,
}

/// A spectral field (3D data) as a trace: one channel per wavelength.
#[derive(Debug, Clone)]
struct FieldEntry {
    desc: Option<FieldDescription>,
    /// Spectrum count stated by the member's header and the first spectrum's grid.
    grid: std::result::Result<(u32, SpectrumGrid), String>,
}

/// One row of the `vendor_peaks` table.
#[derive(Debug, Clone, Copy)]
struct VendorRow {
    trace: u32,
    chrom: usize,
    peak: usize,
}

/// Something `attachments` lists: an audit trail member or a method blob.
#[derive(Debug, Clone)]
enum Attached {
    Audit { item: usize },
    InstrumentMethod(usize),
    ProcessingMethod(usize),
}

/// A Chromeleon 7 archive.
#[derive(Debug)]
pub struct ChromeleonDataset {
    path: PathBuf,
    zip: ZipIndex,
    header: ArchiveHeader,
    sequence_members: Vec<String>,
    signals: Vec<SignalEntry>,
    notes: Vec<String>,
    /// The last decoded signal (trace index, result).
    cache: Option<(u32, std::result::Result<DecodedSignal, SignalError>)>,
    /// The last spectral field read: trace index, inflated member and its record index.
    field_cache: Option<(u32, Vec<u8>, FieldIndex)>,
    /// Signal items with no stored data (derived channels Chromeleon computes on demand).
    derived: usize,
    /// Objects of the sequence files beyond signal descriptions.
    contents: SequenceContents,
    /// Rows of `vendor_peaks`.
    rows: Vec<VendorRow>,
    /// Component names (the `component` column's categories).
    component_names: Vec<String>,
    /// Attachments in listed order.
    attached: Vec<Attached>,
}

/// Local time from a signal's file id (`2025\11\16\164901056.raw` → `2025-11-16T16:49:01.056`),
/// as Chromeleon names the file when the acquisition starts (no time zone).
fn file_id_time(id: &str) -> Option<String> {
    let parts: Vec<&str> = id.split('\\').collect();
    let [y, m, d, rest] = parts.as_slice() else {
        return None;
    };
    let t = rest.strip_suffix(".raw")?;
    let t = t.split('_').next()?;
    if t.len() != 9 || !t.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let num = |s: &str| s.parse::<u32>().ok();
    let (hh, mm, ss) = (num(&t[0..2])?, num(&t[2..4])?, num(&t[4..6])?);
    // a second folder for the same day is named `DD_2`
    let d = d.split('_').next()?;
    let (yy, mo, dd) = (num(y)?, num(m)?, num(d)?);
    if hh > 23 || mm > 59 || ss > 60 || !(1..=12).contains(&mo) || !(1..=31).contains(&dd) {
        return None;
    }
    Some(format!(
        "{yy:04}-{mo:02}-{dd:02}T{hh:02}:{mm:02}:{ss:02}.{}",
        &t[6..9]
    ))
}

/// The spectrum count and first spectrum's grid of a spectral-field member, from its first
/// bytes only.
fn field_grid(
    zip: &ZipIndex,
    member: Option<&str>,
) -> std::result::Result<(u32, SpectrumGrid), String> {
    let m = member
        .and_then(|n| zip.get(n).cloned())
        .ok_or_else(|| "member missing".to_string())?;
    let head = zip.read_prefix(&m, 256 << 10).map_err(|e| e.to_string())?;
    let idx = index_field(&head, false).map_err(|e| e.to_string())?;
    let &(_, a, b) = idx
        .records
        .first()
        .ok_or_else(|| "no spectrum in the first 256 KiB".to_string())?;
    let (grid, _) = decode_spectrum(&head[a..b]).map_err(|e| e.to_string())?;
    Ok((idx.stated, grid))
}

/// The encoding of a signal member: the tag of its first section after `SignHdr`.
fn signal_encoding(zip: &ZipIndex, member: Option<&str>) -> Option<String> {
    let m = zip.get(member?)?.clone();
    let head = zip.read_prefix(&m, 4096).ok()?;
    let mut off = 0usize;
    while off + 16 <= head.len() {
        let tag = &head[off..off + 8];
        if tag != b"SignHdr\0" {
            return Some(
                String::from_utf8_lossy(tag)
                    .trim_end_matches('\0')
                    .to_string(),
            );
        }
        let len = u64::from_le_bytes(head[off + 8..off + 16].try_into().ok()?);
        off = off.checked_add(usize::try_from(len).ok()?.max(16))?;
    }
    None
}

/// The wavelength of a description label `WVL:254 nm`.
fn wavelength_of(label: &str) -> Option<f64> {
    label
        .strip_prefix("WVL:")?
        .trim()
        .trim_end_matches("nm")
        .trim()
        .parse()
        .ok()
}

/// Does a file id's local time name the injection's start (its stored inject time, local
/// clock, within 10 minutes)? `None` when either is missing.
fn file_id_is_start(file_id: &str, inject_time: Option<&str>) -> Option<bool> {
    let f = iso8601_to_unix(&format!("{}Z", file_id_time(file_id)?))?;
    // the inject time's local clock: drop its offset
    let t = inject_time?;
    let local = t.get(..19)?;
    let i = iso8601_to_unix(&format!("{local}Z"))?;
    Some((f - i).abs() <= 600.0)
}

/// The data-vault number of an item from its URL (`…/995.smp` → 995).
fn vault_number(url: Option<&str>) -> Option<u64> {
    url?.rsplit('/').next()?.strip_suffix(".smp")?.parse().ok()
}

/// The sequence file's details of a header.xml injection item: by data-vault number, else by a
/// unique name.
fn injection_details(header: &ArchiveHeader, item: usize, c: &SequenceContents) -> Option<usize> {
    let it = &header.items[item];
    if let Some(n) = vault_number(it.url.as_deref()) {
        return c.injections.iter().position(|d| d.number == Some(n));
    }
    let mut hits = c
        .injections
        .iter()
        .enumerate()
        .filter(|(_, d)| d.name == it.name);
    match (hits.next(), hits.next()) {
        (Some((i, _)), None) => Some(i),
        _ => None,
    }
}

/// The name of the processing-method component a stored peak was identified as.
fn component_of(c: &SequenceContents, p: &StoredPeak) -> Option<String> {
    let key = p.component_key?;
    c.components
        .iter()
        .find(|k| k.key == Some(key))
        .map(|k| k.name.clone())
}

/// All three per-injection factors when they are equal (then the dilution factor and the
/// weight both have that value, whichever field is which).
fn equal_factor(f: &[Option<f64>; 3]) -> Option<f64> {
    match f {
        // exactly equal as stored
        [Some(a), Some(b), Some(c)] if a.to_bits() == b.to_bits() && b.to_bits() == c.to_bits() => {
            Some(*a)
        }
        _ => None,
    }
}

/// Derived quantities of a stored peak from its stored 50/10/5 % points.
#[derive(Debug, Clone, Copy, Default)]
struct PeakShape {
    width_50: Option<f64>,
    width_10: Option<f64>,
    width_5: Option<f64>,
    asymmetry_10: Option<f64>,
    tailing_5: Option<f64>,
    plates: Option<f64>,
}

fn shape(p: &StoredPeak) -> PeakShape {
    let w = |a: Option<(f64, f64)>, b: Option<(f64, f64)>| Some(b?.0 - a?.0).filter(|x| *x > 0.0);
    let apex = p.apex.0;
    let width_50 = w(p.leading.at_50, p.trailing.at_50);
    let asymmetry_10 = match (p.leading.at_10, p.trailing.at_10) {
        (Some(a), Some(b)) if apex > a.0 => Some((b.0 - apex) / (apex - a.0)),
        _ => None,
    };
    let tailing_5 = match (p.leading.at_5, p.trailing.at_5) {
        (Some(a), Some(b)) if apex > a.0 => Some((b.0 - a.0) / (2.0 * (apex - a.0))),
        _ => None,
    };
    PeakShape {
        width_50,
        width_10: w(p.leading.at_10, p.trailing.at_10),
        width_5: w(p.leading.at_5, p.trailing.at_5),
        asymmetry_10,
        tailing_5,
        plates: width_50.map(|w| 5.54 * (p.retention_min / w).powi(2)),
    }
}

impl ChromeleonDataset {
    /// Open an archive by path.
    ///
    /// # Errors
    /// Not a zip, no `header.xml`, or a `header.xml` that is not Chromeleon's.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        Self::open_in(fs, path)
    }

    fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let zip = ZipIndex::open(fs, path, CHROMELEON_ID)?.with_max_member(MAX_SIGNAL_BYTES);
        let hm = zip.get("header.xml").cloned().ok_or_else(|| {
            Error::corrupt(
                CHROMELEON_ID,
                "no header.xml: not a Chromeleon archive, or an incomplete one",
            )
        })?;
        if hm.size > MAX_HEADER_BYTES {
            return Err(Error::unsupported(
                CHROMELEON_ID,
                format!("a header.xml of {} bytes", hm.size),
                "Archives with more items than this are not read; export the sequence in smaller parts.",
            ));
        }
        let text = openreadout_core::zip::text(&zip.read(&hm)?);
        let header = parse_header(&text).map_err(|e| Error::corrupt(CHROMELEON_ID, e))?;
        let mut notes = Vec::new();
        // the sequence files' signal descriptions, keyed by file id
        let mut descs: BTreeMap<String, SignalDescription> = BTreeMap::new();
        let mut fdescs: BTreeMap<String, FieldDescription> = BTreeMap::new();
        let mut sequence_members = Vec::new();
        let mut contents = SequenceContents::default();
        for i in header.of_type(SEQUENCE_ITEM) {
            let Some(member) = header.items[i].member.clone() else {
                continue;
            };
            match zip.get(&member).cloned() {
                None => notes.push(format!(
                    "sequence file {member} is not in the archive: trace grids come from the signals themselves"
                )),
                Some(m) if m.size > MAX_SEQUENCE_BYTES => notes.push(format!(
                    "sequence file {member} ({} bytes) is larger than {MAX_SEQUENCE_BYTES} and was not read",
                    m.size
                )),
                Some(m) => match zip.read(&m) {
                    Ok(b) => {
                        for d in parse_sequence_signals(&b) {
                            descs.entry(d.file_id.clone()).or_insert(d);
                        }
                        for d in parse_sequence_fields(&b) {
                            fdescs.entry(d.file_id.clone()).or_insert(d);
                        }
                        let c = parse_sequence_contents(&b);
                        contents.objects.extend(c.objects);
                        contents.injections.extend(c.injections);
                        contents.chromatograms.extend(c.chromatograms);
                        contents.components.extend(c.components);
                        contents.instrument_methods.extend(c.instrument_methods);
                        contents.processing_methods.extend(c.processing_methods);
                        contents.notes.extend(c.notes);
                    }
                    Err(e) => notes.push(format!("sequence file {member} was not read: {e}")),
                },
            }
            sequence_members.push(member);
        }
        let injections: Vec<usize> = header.of_type(INJECTION_ITEM).collect();
        let mut signals = Vec::new();
        let mut derived = 0usize;
        for (i, it) in header.items.iter().enumerate() {
            let is_field = it.item_type == SPECTRAL_FIELD_ITEM;
            if it.item_type != SIGNAL_ITEM && !is_field {
                continue;
            }
            if it.member.is_none() {
                // a channel Chromeleon derives on demand (e.g. an extracted-ion chromatogram
                // of MS data): no stored points
                derived += 1;
                continue;
            }
            let injection = header.ancestor(i, INJECTION_ITEM);
            let details = injection.and_then(|j| injection_details(&header, j, &contents));
            if is_field {
                let desc = it.file_id.as_ref().and_then(|f| fdescs.get(f)).cloned();
                signals.push(SignalEntry {
                    item: i,
                    injection,
                    injection_index: injection
                        .and_then(|j| injections.iter().position(|&k| k == j)),
                    sequence: header.ancestor(i, SEQUENCE_ITEM),
                    desc: None,
                    decoded: None,
                    details,
                    chrom: None,
                    field: Some(FieldEntry {
                        desc,
                        grid: field_grid(&zip, it.member.as_deref()),
                    }),
                    encoding: Some("3DRawSpc".into()),
                });
                continue;
            }
            let desc = it.file_id.as_ref().and_then(|f| descs.get(f)).cloned();
            let usable = desc
                .as_ref()
                .is_some_and(|d| d.points.is_some_and(|n| n > 0) && d.time.max >= d.time.min);
            let decoded = if usable {
                None
            } else {
                Some(Self::decode_member(&zip, it.member.as_deref()).map_err(|e| e.to_string()))
            };
            let chrom = it.file_id.as_ref().and_then(|f| {
                contents
                    .chromatograms
                    .iter()
                    .position(|c| c.signal_file_id.as_deref() == Some(f.as_str()))
            });
            signals.push(SignalEntry {
                item: i,
                injection,
                injection_index: injection.and_then(|j| injections.iter().position(|&k| k == j)),
                sequence: header.ancestor(i, SEQUENCE_ITEM),
                desc: if usable { desc } else { None },
                decoded,
                details,
                chrom,
                field: None,
                encoding: signal_encoding(&zip, it.member.as_deref()),
            });
        }
        if derived > 0 {
            notes.push(format!(
                "{derived} signal(s) have no stored points (channels Chromeleon derives on demand, e.g. extracted-ion chromatograms of MS data) and are not traces"
            ));
        }
        notes.extend(contents.notes.iter().cloned());
        // vendor_peaks rows: every stored peak of every trace's chromatogram, in trace order
        let mut rows = Vec::new();
        let mut component_names: Vec<String> = Vec::new();
        let mut not_decoded = Vec::new();
        for (t, sig) in signals.iter().enumerate() {
            let Some(ci) = sig.chrom else { continue };
            match &contents.chromatograms[ci].results {
                Ok(Some(r)) => {
                    for (k, p) in r.peaks.iter().enumerate() {
                        rows.push(VendorRow {
                            trace: t as u32,
                            chrom: ci,
                            peak: k,
                        });
                        if let Some(n) = component_of(&contents, p)
                            && !component_names.contains(&n)
                        {
                            component_names.push(n);
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => not_decoded.push(format!("{}: {e}", header.items[sig.item].name)),
            }
        }
        if !not_decoded.is_empty() {
            notes.push(format!(
                "Chromeleon's stored results of {} chromatogram(s) were not decoded (their peaks are not in vendor_peaks): {}",
                not_decoded.len(),
                not_decoded.join("; ")
            ));
        }
        let mut attached = Vec::new();
        for i in header.of_type(AUDIT_TRAIL_ITEM) {
            if header.items[i].member.is_some() {
                attached.push(Attached::Audit { item: i });
            }
        }
        attached.extend((0..contents.instrument_methods.len()).map(Attached::InstrumentMethod));
        attached.extend(
            contents
                .processing_methods
                .iter()
                .enumerate()
                .filter(|(_, (_, b))| b.is_some())
                .map(|(i, _)| Attached::ProcessingMethod(i)),
        );
        let ms: Vec<&str> = header
            .of_type(MS_RAW_ITEM)
            .filter_map(|i| header.items[i].member.as_deref())
            .collect();
        if !ms.is_empty() {
            notes.push(format!(
                "mass-spectrometry data is an embedded Thermo .raw file ({}): it is not read here; extract the member (e.g. `unzip -j '{}' '{}'`) and open it (format thermo-raw)",
                ms.join(", "),
                path.display(),
                ms[0]
            ));
        }
        let mut other: BTreeMap<&str, usize> = BTreeMap::new();
        for it in &header.items {
            if it.member.is_some()
                && ![SIGNAL_ITEM, SEQUENCE_ITEM, MS_RAW_ITEM].contains(&it.item_type.as_str())
            {
                *other.entry(it.item_type.as_str()).or_default() += 1;
            }
        }
        if !other.is_empty() {
            notes.push(format!(
                "listed, not decoded: {}",
                other
                    .iter()
                    .map(|(k, n)| format!("{n} × {}", k.rsplit('.').next().unwrap_or(k)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if signals.iter().any(|s| s.desc.is_none()) {
            notes.push(
                "some signals have no description in the sequence file: their unit is unknown and their grid comes from the signal member".into(),
            );
        }
        Ok(ChromeleonDataset {
            path: path.to_path_buf(),
            zip,
            header,
            sequence_members,
            signals,
            notes,
            cache: None,
            field_cache: None,
            derived,
            contents,
            rows,
            component_names,
            attached,
        })
    }

    fn decode_member(
        zip: &ZipIndex,
        member: Option<&str>,
    ) -> std::result::Result<DecodedSignal, SignalError> {
        let name = member.ok_or_else(|| SignalError::Corrupt("the item names no member".into()))?;
        let m = zip
            .get(name)
            .cloned()
            .ok_or_else(|| SignalError::Corrupt(format!("member {name} is not in the archive")))?;
        if m.size > MAX_SIGNAL_BYTES {
            return Err(SignalError::Unsupported(format!(
                "member {name} is {} bytes (larger than {MAX_SIGNAL_BYTES})",
                m.size
            )));
        }
        let bytes = zip
            .read(&m)
            .map_err(|e| SignalError::Corrupt(e.to_string()))?;
        decode_signal(&bytes).map_err(|e| match e {
            SignalError::Corrupt(s) => SignalError::Corrupt(format!("{name}: {s}")),
            SignalError::Unsupported(s) => SignalError::Unsupported(format!("{name}: {s}")),
        })
    }

    fn name_of(&self, s: &SignalEntry) -> String {
        let it = &self.header.items[s.item];
        match (s.injection, &it.fixed_injection) {
            (Some(j), _) => format!("{} / {}", self.header.items[j].name, it.name),
            (None, Some(f)) => format!("{f} / {}", it.name),
            (None, None) => it.name.clone(),
        }
    }

    /// Grid of a signal: (first time min, step min, points).
    fn grid(s: &SignalEntry) -> Option<(f64, f64, u64)> {
        if let Some(d) = &s.desc {
            let n = d.points?;
            let step = if n > 1 {
                (d.time.max - d.time.min) / (n - 1) as f64
            } else {
                0.0
            };
            return Some((d.time.min, step, n));
        }
        match &s.decoded {
            Some(Ok(d)) => Some((d.start_min, d.step_min(), d.len() as u64)),
            _ => None,
        }
    }

    fn trace_info(&self, index: u32, s: &SignalEntry) -> TraceInfo {
        let it = &self.header.items[s.item];
        let mut extra: BTreeMap<String, Value> = BTreeMap::new();
        extra.insert("signal".into(), json!(it.name));
        if let Some(j) = s.injection {
            let inj = &self.header.items[j];
            extra.insert("injection".into(), json!(inj.name));
            if let Some(t) = &inj.injection_type {
                extra.insert("injection_type".into(), json!(t));
            }
        } else if let Some(f) = &it.fixed_injection {
            // a calibration standard's signal kept by a processing method
            extra.insert("injection".into(), json!(f));
            extra.insert("fixed_injection".into(), json!(true));
            if let Some(p) = self.header.ancestor(s.item, PROCESSING_METHOD_ITEM) {
                extra.insert("processing_method".into(), json!(self.header.items[p].name));
            }
        }
        if let Some(k) = s.injection_index {
            extra.insert("injection_index".into(), json!(k));
        }
        if let Some(d) = s.details.and_then(|i| self.contents.injections.get(i)) {
            let mut put = |k: &str, v: Option<Value>| {
                if let Some(v) = v {
                    extra.insert(k.into(), v);
                }
            };
            put("injection_position", d.position.clone().map(Value::from));
            put("injection_volume_ul", d.volume_ul.map(Value::from));
            put("inject_time", d.inject_time.clone().map(Value::from));
            put("injection_status", d.status.clone().map(Value::from));
            put("calibration_level", d.level.clone().map(Value::from));
            put(
                "processing_method",
                d.processing_method.clone().map(Value::from),
            );
            put(
                "instrument_method",
                d.instrument_method.clone().map(Value::from),
            );
            if let Some(f) = equal_factor(&d.factors) {
                put("dilution_factor", Some(json!(f)));
                put("weight", Some(json!(f)));
            }
        }
        if let Some(c) = s.chrom.and_then(|i| self.contents.chromatograms.get(i)) {
            match &c.results {
                Ok(Some(r)) => {
                    extra.insert("vendor_results".into(), json!("stored"));
                    extra.insert("vendor_peaks".into(), json!(r.peaks.len()));
                }
                Ok(None) => {
                    extra.insert("vendor_results".into(), json!("none"));
                }
                Err(e) => {
                    extra.insert("vendor_results".into(), json!(format!("not decoded: {e}")));
                }
            }
        }
        if let Some(q) = s.sequence {
            extra.insert("sequence".into(), json!(self.header.items[q].name));
        }
        if let Some(m) = &it.member {
            extra.insert("member".into(), json!(m));
        }
        if let Some(f) = &it.file_id {
            extra.insert("file_id".into(), json!(f));
            // an injection's file id names its local start; a calibration standard copied into a
            // processing method gets a new id (the copy time)
            // (not when the injection's own inject time says the id is not its start: ids of data
            // copied later name the copy time)
            let inject = s
                .details
                .and_then(|i| self.contents.injections.get(i))
                .and_then(|d| d.inject_time.as_deref());
            if let Some(t) = file_id_time(f)
                .filter(|_| s.injection.is_some())
                .filter(|_| file_id_is_start(f, inject) != Some(false))
            {
                extra.insert("acquired_local".into(), json!(t));
            }
        }
        if let Some(f) = &s.field {
            return self.field_trace_info(index, s, f, extra);
        }
        let (unit, scale) = match &s.desc {
            Some(d) => {
                if let Some(v) = &d.device {
                    extra.insert("device".into(), json!(v));
                }
                if let Some(v) = &d.module {
                    extra.insert("module".into(), json!(v));
                }
                if let Some(v) = &d.signal.quantity {
                    extra.insert("quantity".into(), json!(v));
                }
                if let Some(v) = &d.label {
                    extra.insert("label".into(), json!(v));
                    if let Some(w) = wavelength_of(v) {
                        extra.insert("wavelength_nm".into(), json!(w));
                    }
                }
                extra.insert("stored_min".into(), json!(d.signal.min));
                extra.insert("stored_max".into(), json!(d.signal.max));
                (d.signal.unit.clone(), d.scale.unwrap_or(1.0))
            }
            None => match &s.decoded {
                Some(Ok(d)) => (None, d.scale()),
                _ => (None, 1.0),
            },
        };
        if let Some(Err(e)) = &s.decoded {
            extra.insert("not_decoded".into(), json!(e));
        }
        let encoding = s.encoding.clone().unwrap_or_else(|| "unknown".into());
        extra.insert("encoding".into(), json!(encoding));
        let float = encoding == "PtsDDCmp";
        let irregular = s.desc.as_ref().is_some_and(|d| !d.is_regular())
            || matches!(&s.decoded, Some(Ok(d)) if d.times.is_some());
        let (rate, start_s, n) = match Self::grid(s) {
            Some((first, step, n)) => {
                extra.insert("x_start_min".into(), json!(tidy(first)));
                extra.insert(
                    "x_end_min".into(),
                    json!(tidy(first + step * n.saturating_sub(1) as f64)),
                );
                if irregular {
                    (0.0, Some(tidy(first * 60.0)), n)
                } else {
                    extra.insert(
                        "axis".into(),
                        json!({"quantity": "retention_time", "unit": "min", "first": tidy(first), "step": tidy(step)}),
                    );
                    (
                        if step > 0.0 {
                            tidy(1.0 / (step * 60.0))
                        } else {
                            0.0
                        },
                        Some(tidy(first * 60.0)),
                        n,
                    )
                }
            }
            None => (0.0, None, 0),
        };
        let value = SignalChannelInfo {
            index: 0,
            name: it.name.clone(),
            unit,
            dtype: if float { "float64" } else { "int64" }.into(),
            scale: if float { 1.0 } else { scale },
            offset: 0.0,
            extra: BTreeMap::new(),
        };
        let channels = if irregular {
            extra.insert("irregular_sampling".into(), json!(true));
            extra.insert(
                "time_channel".into(),
                json!("the points are not evenly spaced: channel 0 holds each point's retention time (min)"),
            );
            vec![
                SignalChannelInfo {
                    index: 0,
                    name: "time".into(),
                    unit: Some("min".into()),
                    dtype: "float64".into(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: BTreeMap::new(),
                },
                SignalChannelInfo { index: 1, ..value },
            ]
        } else {
            vec![value]
        };
        TraceInfo {
            index,
            name: Some(self.name_of(s)),
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: 1,
            channels,
            start_s,
            extra,
        }
    }

    /// A spectral field as a trace: one channel per wavelength of its first spectrum's grid,
    /// one sample per spectrum.
    fn field_trace_info(
        &self,
        index: u32,
        signal: &SignalEntry,
        field: &FieldEntry,
        mut extra: BTreeMap<String, Value>,
    ) -> TraceInfo {
        extra.insert("encoding".into(), json!("3DRawSpc"));
        extra.insert("spectral_field".into(), json!(true));
        let unit = field.desc.as_ref().and_then(|desc| desc.value.unit.clone());
        if let Some(desc) = &field.desc {
            if let Some(v) = &desc.device {
                extra.insert("device".into(), json!(v));
            }
            if let Some(v) = &desc.module {
                extra.insert("module".into(), json!(v));
            }
            extra.insert("stored_min".into(), json!(desc.value.min));
            extra.insert("stored_max".into(), json!(desc.value.max));
            extra.insert(
                "wavelength_range_nm".into(),
                json!([desc.wavelength.min, desc.wavelength.max]),
            );
        }
        let (n, grid) = match &field.grid {
            Ok((n, wl_grid)) => (u64::from(*n), Some(*wl_grid)),
            Err(e) => {
                extra.insert("not_decoded".into(), json!(e));
                (0, None)
            }
        };
        let (mut rate, mut start_s) = (0.0, None);
        if let Some(desc) = &field.desc
            && n > 1
            && desc.time.max > desc.time.min
        {
            let step = (desc.time.max - desc.time.min) / (n - 1) as f64;
            rate = tidy(1.0 / (step * 60.0));
            start_s = Some(tidy(desc.time.min * 60.0));
            extra.insert("x_start_min".into(), json!(tidy(desc.time.min)));
            extra.insert("x_end_min".into(), json!(tidy(desc.time.max)));
            extra.insert(
                "axis".into(),
                json!({"quantity": "retention_time", "unit": "min", "first": tidy(desc.time.min), "step": tidy(step)}),
            );
        }
        let channels = grid.map_or_else(Vec::new, |wl_grid| {
            extra.insert("wavelength_step_nm".into(), json!(tidy(wl_grid.step())));
            (0..wl_grid.points)
                .map(|c| {
                    let w = tidy(wl_grid.x0 + c as f64 * wl_grid.step());
                    let mut e = BTreeMap::new();
                    e.insert("wavelength_nm".into(), json!(w));
                    SignalChannelInfo {
                        index: c as u32,
                        name: format!("{w} nm"),
                        unit: unit.clone(),
                        dtype: "int64".into(),
                        scale: wl_grid.scale(),
                        offset: 0.0,
                        extra: e,
                    }
                })
                .collect()
        });
        TraceInfo {
            index,
            name: Some(self.name_of(signal)),
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: 1,
            channels,
            start_s,
            extra,
        }
    }

    /// The stored results and peak of a `vendor_peaks` row.
    fn row_peak(&self, r: &VendorRow) -> Option<(&StoredResults, &StoredPeak)> {
        let res = self
            .contents
            .chromatograms
            .get(r.chrom)?
            .results
            .as_ref()
            .ok()?
            .as_ref()?;
        Some((res, res.peaks.get(r.peak)?))
    }

    /// Signal unit of a trace (from the sequence file's description).
    fn unit_of(&self, trace: u32) -> Option<String> {
        self.signals
            .get(trace as usize)
            .and_then(|s| s.desc.as_ref())
            .and_then(|d| d.signal.unit.clone())
    }

    /// Tables: `vendor_peaks` (when Chromeleon saved results), then `injections`.
    fn table_infos(&self) -> Vec<TableInfo> {
        let mut out = Vec::new();
        if let Some(t) = self.vendor_table_info(out.len() as u32) {
            out.push(t);
        }
        if let Some(t) = self.injection_table_info(out.len() as u32) {
            out.push(t);
        }
        out
    }

    fn vendor_table_info(&self, index: u32) -> Option<TableInfo> {
        if self.rows.is_empty() {
            return None;
        }
        let units: Vec<Option<String>> = {
            let mut u: Vec<Option<String>> =
                self.rows.iter().map(|r| self.unit_of(r.trace)).collect();
            u.dedup();
            u
        };
        let unit = match units.as_slice() {
            [Some(u)] => Some(u.clone()),
            _ => None,
        };
        let min = || Some("min".to_string());
        let col = |i: u32, name: &str, unit: Option<String>, label: &str, dtype: &str| ColumnInfo {
            index: i,
            name: name.into(),
            label: Some(label.into()),
            dtype: dtype.into(),
            unit,
            range: None,
            extra: BTreeMap::new(),
        };
        let mut columns = vec![
            col(
                0,
                "trace",
                None,
                "index of the trace (traces[]) Chromeleon integrated",
                "uint32",
            ),
            col(
                1,
                "peak",
                None,
                "peak number within its chromatogram, in stored order",
                "uint32",
            ),
            col(
                2,
                "rt_min",
                min(),
                "retention time (Chromeleon's)",
                "float64",
            ),
            col(3, "start_min", min(), "start of the integration", "float64"),
            col(4, "end_min", min(), "end of the integration", "float64"),
            col(
                5,
                "baseline_start",
                unit.clone(),
                "the baseline at the start (the skim line for a rider)",
                "float64",
            ),
            col(
                6,
                "baseline_end",
                unit.clone(),
                "the baseline at the end",
                "float64",
            ),
            col(
                7,
                "area",
                unit.as_ref().map(|u| format!("{u}*min")),
                "area (Chromeleon's)",
                "float64",
            ),
            col(
                8,
                "height",
                unit.clone(),
                "height above the baseline at the apex (Chromeleon's)",
                "float64",
            ),
            col(
                9,
                "area_pct",
                Some("%".into()),
                "100 × area / sum of the chromatogram's stored peak areas (computed from the stored areas)",
                "float64",
            ),
            col(
                10,
                "component",
                None,
                "processing-method component the peak was identified as (code into extra.categories)",
                "uint32",
            ),
            col(
                11,
                "expected_rt_min",
                min(),
                "the component's expected retention time",
                "float64",
            ),
            col(
                12,
                "skim",
                None,
                "1-based skim line the peak rides on (a rider; 0: none)",
                "uint32",
            ),
            col(
                13,
                "code",
                None,
                "Chromeleon's stored code word (bit meanings not established)",
                "uint32",
            ),
            col(
                14,
                "width_50_min",
                min(),
                "width at 50 % height, from the stored 50 % points",
                "float64",
            ),
            col(
                15,
                "width_10_min",
                min(),
                "width at 10 % height, from the stored 10 % points",
                "float64",
            ),
            col(
                16,
                "width_5_min",
                min(),
                "width at 5 % height, from the stored 5 % points",
                "float64",
            ),
            col(
                17,
                "asymmetry_10",
                None,
                "asymmetry b/a at 10 % height (Ph. Eur.), from the stored points",
                "float64",
            ),
            col(
                18,
                "tailing_5",
                None,
                "tailing factor W0.05/2f at 5 % height (USP), from the stored points",
                "float64",
            ),
            col(
                19,
                "plates_ep",
                None,
                "plates 5.54 (tR/W0.5)² (Ph. Eur.), from the stored points",
                "float64",
            ),
            col(
                20,
                "resolution_ep",
                None,
                "resolution to the next peak by retention time, 1.18 Δt/(W0.5,1 + W0.5,2) (Ph. Eur.), from the stored points",
                "float64",
            ),
            col(
                21,
                "apex_value",
                unit.clone(),
                "signal value at the apex",
                "float64",
            ),
        ];
        columns[10]
            .extra
            .insert("categories".into(), json!(self.component_names));
        let mut extra = BTreeMap::new();
        extra.insert("source".into(), json!("vendor"));
        extra.insert(
            "results".into(),
            json!("Chromeleon's integration results as it last saved them with each chromatogram (retention time, limits, baseline, area, height, the 50/10/5 % points); widths, asymmetry, tailing, plates and resolution are computed from those stored points with the pharmacopoeial formulas named in each column, not read"),
        );
        let traces: Vec<u32> = {
            let mut t: Vec<u32> = self.rows.iter().map(|r| r.trace).collect();
            t.dedup();
            t
        };
        extra.insert("traces".into(), json!(traces));
        let mut versions: Vec<u8> = self
            .rows
            .iter()
            .filter_map(|r| self.row_peak(r).map(|(res, _)| res.version))
            .collect();
        versions.sort_unstable();
        versions.dedup();
        extra.insert("results_versions".into(), json!(versions));
        if unit.is_none() {
            extra.insert(
                "units".into(),
                json!("area and height are in each trace's unit (traces[].channels[0].unit), area × min"),
            );
        }
        Some(TableInfo {
            index,
            name: Some("vendor_peaks".into()),
            row_count: self.rows.len() as u64,
            columns,
            extra,
        })
    }

    /// Category lists of the `injections` table: (name, type, status, position, level,
    /// processing method, instrument method).
    fn injection_categories(&self) -> [Vec<String>; 7] {
        let mut c: [Vec<String>; 7] = Default::default();
        let mut add = |k: usize, v: Option<&str>| {
            if let Some(v) = v
                && !c[k].iter().any(|x| x == v)
            {
                c[k].push(v.to_string());
            }
        };
        for d in &self.contents.injections {
            add(0, Some(d.name.as_str()));
            add(1, d.injection_type.as_deref());
            add(2, d.status.as_deref());
            add(3, d.position.as_deref());
            add(4, d.level.as_deref());
            add(5, d.processing_method.as_deref());
            add(6, d.instrument_method.as_deref());
        }
        c
    }

    fn injection_table_info(&self, index: u32) -> Option<TableInfo> {
        if self.contents.injections.is_empty() {
            return None;
        }
        let cats = self.injection_categories();
        let col = |i: u32, name: &str, unit: Option<&str>, label: &str, dtype: &str| ColumnInfo {
            index: i,
            name: name.into(),
            label: Some(label.into()),
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let mut columns = vec![
            col(
                0,
                "injection",
                None,
                "0-based position in the sequence",
                "uint32",
            ),
            col(
                1,
                "name",
                None,
                "injection name (code into extra.categories)",
                "uint32",
            ),
            col(
                2,
                "injection_type",
                None,
                "Unknown, Standard, Blank, ... (code into extra.categories)",
                "uint32",
            ),
            col(
                3,
                "status",
                None,
                "Finished, Interrupted, ... (code into extra.categories)",
                "uint32",
            ),
            col(
                4,
                "position",
                None,
                "autosampler position (code into extra.categories)",
                "uint32",
            ),
            col(5, "volume_ul", Some("µL"), "injection volume", "float64"),
            col(
                6,
                "inject_time",
                Some("s"),
                "inject time, seconds since 1970-01-01 UTC (extra.inject_times: as stored, with the local offset)",
                "float64",
            ),
            col(
                7,
                "level",
                None,
                "calibration level (code into extra.categories)",
                "uint32",
            ),
            col(
                8,
                "processing_method",
                None,
                "code into extra.categories",
                "uint32",
            ),
            col(
                9,
                "instrument_method",
                None,
                "code into extra.categories",
                "uint32",
            ),
            col(
                10,
                "dilution_factor",
                None,
                "dilution factor; given only when the three per-injection factors are equal (then dilution factor and weight are that value)",
                "float64",
            ),
            col(
                11,
                "weight",
                None,
                "sample weight factor; given only when the three per-injection factors are equal",
                "float64",
            ),
            col(
                12,
                "vault_number",
                None,
                "the injection's number in the data vault (NNN.smp)",
                "uint32",
            ),
        ];
        for (k, c) in [
            (1usize, 0usize),
            (2, 1),
            (3, 2),
            (4, 3),
            (7, 4),
            (8, 5),
            (9, 6),
        ] {
            columns[k].extra.insert("categories".into(), json!(cats[c]));
        }
        let mut extra = BTreeMap::new();
        extra.insert(
            "inject_times".into(),
            json!(
                self.contents
                    .injections
                    .iter()
                    .map(|d| d.inject_time.clone())
                    .collect::<Vec<_>>()
            ),
        );
        Some(TableInfo {
            index,
            name: Some("injections".into()),
            row_count: self.contents.injections.len() as u64,
            columns,
            extra,
        })
    }

    fn vendor_rows(&self, start: usize, end: usize) -> Vec<Vec<f64>> {
        let o = |v: Option<f64>| v.unwrap_or(f64::NAN);
        let mut cols: Vec<Vec<f64>> = (0..22).map(|_| Vec::with_capacity(end - start)).collect();
        for r in &self.rows[start..end] {
            let Some((res, p)) = self.row_peak(r) else {
                for c in &mut cols {
                    c.push(f64::NAN);
                }
                continue;
            };
            let total: f64 = res.peaks.iter().map(|q| q.area).sum();
            let sh = shape(p);
            // resolution to the next peak by retention time
            let next = res
                .peaks
                .iter()
                .filter(|q| q.retention_min > p.retention_min)
                .min_by(|a, b| a.retention_min.total_cmp(&b.retention_min));
            let resolution = match (sh.width_50, next.and_then(|q| shape(q).width_50)) {
                (Some(w1), Some(w2)) => {
                    next.map(|q| 1.18 * (q.retention_min - p.retention_min) / (w1 + w2))
                }
                _ => None,
            };
            let line = res.baseline_line(p);
            let comp = component_of(&self.contents, p)
                .and_then(|n| self.component_names.iter().position(|x| *x == n))
                .map(|i| i as f64);
            let vals = [
                f64::from(r.trace),
                (r.peak + 1) as f64,
                p.retention_min,
                p.start.0,
                p.end.0,
                o(line.map(|l| l.0.1)),
                o(line.map(|l| l.1.1)),
                p.area,
                p.height,
                if total > 0.0 {
                    100.0 * p.area / total
                } else {
                    f64::NAN
                },
                o(comp),
                o(p.component_values.map(|v| v.0)),
                f64::from(p.skim),
                f64::from(p.code),
                o(sh.width_50),
                o(sh.width_10),
                o(sh.width_5),
                o(sh.asymmetry_10),
                o(sh.tailing_5),
                o(sh.plates),
                o(resolution),
                p.apex.1,
            ];
            for (c, v) in cols.iter_mut().zip(vals) {
                c.push(v);
            }
        }
        cols
    }

    fn injection_rows(&self, start: usize, end: usize) -> Vec<Vec<f64>> {
        let cats = self.injection_categories();
        let code = |k: usize, v: Option<&str>| {
            v.and_then(|v| cats[k].iter().position(|x| x == v))
                .map_or(f64::NAN, |i| i as f64)
        };
        let o = |v: Option<f64>| v.unwrap_or(f64::NAN);
        let mut cols: Vec<Vec<f64>> = (0..13).map(|_| Vec::with_capacity(end - start)).collect();
        for (i, d) in self.contents.injections[start..end].iter().enumerate() {
            let f = equal_factor(&d.factors);
            let vals = [
                (start + i) as f64,
                code(0, Some(d.name.as_str())),
                code(1, d.injection_type.as_deref()),
                code(2, d.status.as_deref()),
                code(3, d.position.as_deref()),
                o(d.volume_ul),
                o(d.inject_time.as_deref().and_then(iso8601_to_unix)),
                code(4, d.level.as_deref()),
                code(5, d.processing_method.as_deref()),
                code(6, d.instrument_method.as_deref()),
                o(f),
                o(f),
                o(d.number.map(|n| n as f64)),
            ];
            for (c, v) in cols.iter_mut().zip(vals) {
                c.push(v);
            }
        }
        cols
    }

    /// Decompressed bytes of an attachment.
    fn attachment_bytes(&self, a: &Attached) -> std::result::Result<Vec<u8>, String> {
        match a {
            Attached::Audit { item } => {
                let it = &self.header.items[*item];
                let m = it
                    .member
                    .as_deref()
                    .and_then(|m| self.zip.get(m).cloned())
                    .ok_or_else(|| format!("{}: member missing", it.name))?;
                let raw = self.zip.read(&m).map_err(|e| e.to_string())?;
                unpack_blob(&raw)
            }
            Attached::InstrumentMethod(i) => {
                unpack_blob(&self.contents.instrument_methods[*i].blob)
            }
            Attached::ProcessingMethod(i) => self.contents.processing_methods[*i]
                .1
                .as_deref()
                .ok_or_else(|| "no settings blob".to_string())
                .and_then(unpack_blob),
        }
    }

    fn attachment_name(&self, a: &Attached) -> (String, &'static str) {
        match a {
            Attached::Audit { item } => {
                let inj = self
                    .header
                    .ancestor(*item, INJECTION_ITEM)
                    .map_or_else(String::new, |j| self.header.items[j].name.clone());
                (format!("{inj} / audit trail"), "audit_trail")
            }
            Attached::InstrumentMethod(i) => (
                format!(
                    "{} (instrument method)",
                    self.contents.instrument_methods[*i].name
                ),
                "instrument_method",
            ),
            Attached::ProcessingMethod(i) => (
                format!(
                    "{} (processing method)",
                    self.contents.processing_methods[*i].0
                ),
                "processing_method",
            ),
        }
    }

    /// Inflate and index a spectral field's member (cached for the last one read).
    fn field_member(&mut self, index: u32) -> Result<()> {
        if self.field_cache.as_ref().map(|c| c.0) == Some(index) {
            return Ok(());
        }
        let s = &self.signals[index as usize];
        let name = self.name_of(s);
        let member = self.header.items[s.item].member.clone().unwrap_or_default();
        let m = self.zip.get(&member).cloned().ok_or_else(|| {
            Error::corrupt(
                CHROMELEON_ID,
                format!("{name}: member {member} is not in the archive"),
            )
        })?;
        let bytes = self.zip.read(&m)?;
        let idx = index_field(&bytes, true).map_err(|e| match e {
            SignalError::Corrupt(x) => Error::corrupt(CHROMELEON_ID, format!("{name}: {x}")),
            SignalError::Unsupported(x) => Error::unsupported(
                CHROMELEON_ID,
                format!("{name}: {x}"),
                "This 3D field uses a layout no corpus file validates; export its spectra from Chromeleon.",
            ),
        })?;
        self.field_cache = Some((index, bytes, idx));
        Ok(())
    }

    fn read_field(&mut self, index: u32, first: u64, max: u64) -> Result<Trace> {
        let s = self.signals[index as usize].clone();
        let name = self.name_of(&s);
        let grid = match s.field.as_ref().map(|f| &f.grid) {
            Some(Ok((_, g))) => *g,
            Some(Err(e)) => {
                return Err(Error::corrupt(CHROMELEON_ID, format!("{name}: {e}")));
            }
            None => return Err(Error::corrupt(CHROMELEON_ID, "not a field")),
        };
        self.field_member(index)?;
        let Some((_, bytes, idx)) = &self.field_cache else {
            return Err(Error::corrupt(CHROMELEON_ID, "field cache"));
        };
        let total = idx.records.len() as u64;
        if first > total {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({total} spectra)"
            )));
        }
        let n = max.min(total - first) as usize;
        let mut channels: Vec<Vec<f64>> = vec![Vec::with_capacity(n); grid.points];
        for &(_, a, b) in &idx.records[first as usize..first as usize + n] {
            let (g, raw) = decode_spectrum(&bytes[a..b]).map_err(|e| match e {
                SignalError::Corrupt(x) => Error::corrupt(CHROMELEON_ID, format!("{name}: {x}")),
                SignalError::Unsupported(x) => Error::unsupported(
                    CHROMELEON_ID,
                    format!("{name}: {x}"),
                    "This 3D field uses a layout no corpus file validates.",
                ),
            })?;
            if g != grid {
                return Err(Error::unsupported(
                    CHROMELEON_ID,
                    format!(
                        "{name}: a spectrum on another wavelength grid or scale than the first"
                    ),
                    "Fields whose wavelength grid changes are not decoded.",
                ));
            }
            for (c, v) in channels.iter_mut().zip(&raw) {
                c.push(grid.value(*v));
            }
        }
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample: first,
            channels,
        })
    }

    /// Decode a spectral field whole and compare it with its description.
    fn check_field(&mut self, index: u32, f: &FieldEntry) -> std::result::Result<(), Finding> {
        let name = self.name_of(&self.signals[index as usize]);
        let grid = match &f.grid {
            Ok((_, g)) => *g,
            Err(e) => {
                return Err(Finding::warning(
                    "field_not_decoded",
                    format!("{name}: {e}"),
                ));
            }
        };
        if let Err(e) = self.field_member(index) {
            let code = if e.exit_code() == 6 {
                "field_not_decoded"
            } else {
                "bad_field"
            };
            return Err(if code == "bad_field" {
                Finding::error(code, e.to_string())
            } else {
                Finding::warning(code, e.to_string())
            });
        }
        let Some((_, bytes, idx)) = &self.field_cache else {
            return Err(Finding::error("bad_field", format!("{name}: not read")));
        };
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &(t, a, b) in &idx.records {
            let (g, raw) = decode_spectrum(&bytes[a..b]).map_err(|e| match e {
                SignalError::Corrupt(x) => {
                    Finding::error("bad_field", format!("{name} at {t} min: {x}"))
                }
                SignalError::Unsupported(x) => {
                    Finding::warning("field_not_decoded", format!("{name} at {t} min: {x}"))
                }
            })?;
            if g != grid {
                return Err(Finding::warning(
                    "field_not_decoded",
                    format!(
                        "{name}: the spectrum at {t} min is on another wavelength grid or scale"
                    ),
                ));
            }
            for &v in &raw {
                lo = lo.min(g.value(v));
                hi = hi.max(g.value(v));
            }
        }
        let mut off = Vec::new();
        if idx.records.len() != idx.stated as usize {
            off.push(format!(
                "{} spectra, the header states {}",
                idx.records.len(),
                idx.stated
            ));
        }
        if let Some(d) = &f.desc {
            let (t0, t1) = (
                idx.records.first().map_or(f64::NAN, |r| r.0),
                idx.records.last().map_or(f64::NAN, |r| r.0),
            );
            if (t0 - d.time.min).abs() > 1e-6 || (t1 - d.time.max).abs() > 1e-6 {
                off.push(format!(
                    "spectra from {t0} to {t1} min, the sequence file {}–{}",
                    d.time.min, d.time.max
                ));
            }
            let tol = 1e-9 * d.value.max.abs().max(d.value.min.abs()).max(1.0);
            if (lo - d.value.min).abs() > tol || (hi - d.value.max).abs() > tol {
                off.push(format!(
                    "values {lo}–{hi}, the sequence file {}–{}",
                    d.value.min, d.value.max
                ));
            }
            let w1 = grid.x0 + (grid.points.saturating_sub(1)) as f64 * grid.step();
            if (grid.x0 - d.wavelength.min).abs() > 1e-6 || (w1 - d.wavelength.max).abs() > 1e-6 {
                off.push(format!(
                    "wavelengths {}–{w1} nm, the sequence file {}–{}",
                    grid.x0, d.wavelength.min, d.wavelength.max
                ));
            }
        }
        if off.is_empty() {
            Ok(())
        } else {
            Err(Finding::error(
                "field_mismatch",
                format!("{name}: {}", off.join("; ")),
            ))
        }
    }

    fn decoded(&mut self, index: u32) -> Result<&DecodedSignal> {
        let s = self.signals.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (this archive has {} traces)",
                self.signals.len()
            ))
        })?;
        if self.cache.as_ref().map(|c| c.0) != Some(index) {
            let r = match &s.decoded {
                Some(Ok(d)) => Ok(d.clone()),
                _ => Self::decode_member(&self.zip, self.header.items[s.item].member.as_deref()),
            };
            self.cache = Some((index, r));
        }
        let name = self.name_of(&self.signals[index as usize]);
        match self.cache.as_ref().map(|c| &c.1) {
            Some(Ok(d)) => Ok(d),
            Some(Err(SignalError::Unsupported(e))) => Err(Error::unsupported(
                CHROMELEON_ID,
                format!("{name}: {e}"),
                "This signal uses a layout no corpus file validates; export it from Chromeleon (File > Export > ASCII) and read the export.",
            )),
            Some(Err(SignalError::Corrupt(e))) => {
                Err(Error::corrupt(CHROMELEON_ID, format!("{name}: {e}")))
            }
            None => Err(Error::corrupt(CHROMELEON_ID, "signal cache")),
        }
    }
}

impl Dataset for ChromeleonDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces: Vec<TraceInfo> = self
            .signals
            .iter()
            .enumerate()
            .map(|(i, s)| self.trace_info(i as u32, s))
            .collect();
        let mut notes = self.notes.clone();
        let injections = self.header.of_type(INJECTION_ITEM).count();
        let fixed = self
            .signals
            .iter()
            .filter(|s| {
                s.injection.is_none() && self.header.items[s.item].fixed_injection.is_some()
            })
            .count();
        notes.insert(
            0,
            format!(
                "{} trace(s): every archived 2D signal of {injections} injection(s){}, named `<injection> / <signal>`; values = stored integer × scale, in the unit of the sequence file's signal description",
                traces.len(),
                if fixed > 0 {
                    format!(" and {fixed} calibration-standard signal(s) held by processing methods (extra.fixed_injection)")
                } else {
                    String::new()
                }
            ),
        );
        let stored = self
            .contents
            .chromatograms
            .iter()
            .filter(|c| matches!(c.results, Ok(Some(_))))
            .count();
        if stored > 0 || !self.contents.injections.is_empty() {
            notes.push(format!(
                "Chromeleon's own integration results: {} peak(s) of {stored} chromatogram(s) in tables vendor_peaks (as Chromeleon last saved them; injections without saved results have none); injection details (position, volume, inject time, type, status, methods) in the injections table and traces[].extra; audit trails and methods are attachments (openreadout export --attachment)",
                self.rows.len()
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.zip.file_len,
            format: ChromeleonReader.descriptor(),
            format_version: self.header.generator_version.clone(),
            images: Vec::new(),
            tables: self.table_infos(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let items: Vec<Value> = self
            .header
            .items
            .iter()
            .map(|it| {
                json!({
                    "id": it.id, "name": it.name, "item_type": it.item_type, "url": it.url,
                    "member": it.member, "size": it.size, "file_id": it.file_id,
                    "injection_type": it.injection_type, "parent": it.parent,
                })
            })
            .collect();
        let descriptions: Vec<Value> = self
            .signals
            .iter()
            .filter_map(|s| s.desc.as_ref())
            .map(|d| {
                json!({
                    "file_id": d.file_id, "points": d.points,
                    "time": {"min": d.time.min, "max": d.time.max, "unit": d.time.unit, "quantity": d.time.quantity},
                    "signal": {"min": d.signal.min, "max": d.signal.max, "unit": d.signal.unit, "quantity": d.signal.quantity},
                    "device": d.device, "module": d.module, "scale": d.scale,
                })
            })
            .collect();
        let injections: Vec<Value> = self
            .contents
            .injections
            .iter()
            .map(|d| {
                json!({
                    "name": d.name, "injection_type": d.injection_type, "status": d.status,
                    "position": d.position, "volume_ul": d.volume_ul, "inject_time": d.inject_time,
                    "level": d.level, "factors": d.factors, "processing_method": d.processing_method,
                    "instrument_method": d.instrument_method, "vault_number": d.number,
                })
            })
            .collect();
        let components: Vec<Value> = self
            .contents
            .components
            .iter()
            .map(|c| {
                json!({
                    "processing_method": c.method, "name": c.name, "retention_min": c.retention_min,
                    "second_value": c.second_value, "amount_unit": c.amount_unit,
                    "levels": c.levels.iter().map(|(k, a)| json!({"level": k, "amount": a})).collect::<Vec<_>>(),
                })
            })
            .collect();
        let methods: Vec<Value> = self
            .contents
            .instrument_methods
            .iter()
            .map(|m| {
                match unpack_blob(&m.blob)
                    .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
                    .and_then(|x| method_steps(&x))
                {
                    Ok(steps) => json!({
                        "name": m.name,
                        "steps": steps.iter().map(|st| json!({
                            "stage": st.stage, "time_min": st.time_min, "kind": st.kind,
                            "symbol": st.symbol, "value": st.value,
                        })).collect::<Vec<_>>(),
                    }),
                    Err(e) => json!({"name": m.name, "not_decoded": e}),
                }
            })
            .collect();
        let chromatograms: Vec<Value> = self
            .contents
            .chromatograms
            .iter()
            .map(|c| {
                let (status, peaks, version) = match &c.results {
                    Ok(Some(r)) => ("stored".to_string(), r.peaks.len(), Some(r.version)),
                    Ok(None) => ("none".to_string(), 0, None),
                    Err(e) => (format!("not decoded: {e}"), 0, None),
                };
                json!({"name": c.name, "signal_file_id": c.signal_file_id, "results": status,
                       "peaks": peaks, "version": version})
            })
            .collect();
        Ok(json!({
            "injections": injections,
            "components": components,
            "instrument_methods": methods,
            "chromatograms": chromatograms,
            "generator_version": self.header.generator_version,
            "container_version": self.header.container_version,
            "date_created": self.header.date_created,
            "sequence_files": self.sequence_members,
            "items": items,
            "signal_descriptions": descriptions,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("traces[].name", Source::Inferred),
            ("traces[].sample_count", Source::Inferred),
            ("traces[].sample_rate_hz", Source::Inferred),
            ("traces[].start_s", Source::Inferred),
            ("traces[].channels[].unit", Source::Inferred),
            ("traces[].channels[].scale", Source::Inferred),
            ("traces[].extra.injection", Source::Inferred),
            ("traces[].extra.injection_type", Source::Inferred),
            ("traces[].extra.sequence", Source::Inferred),
            ("traces[].extra.acquired_local", Source::Inferred),
            ("traces[].extra.device", Source::Inferred),
            ("traces[].extra.module", Source::Inferred),
            ("traces[].extra.stored_min", Source::Inferred),
            ("traces[].extra.stored_max", Source::Inferred),
            ("traces[].extra.injection_position", Source::Inferred),
            ("traces[].extra.injection_volume_ul", Source::Inferred),
            ("traces[].extra.inject_time", Source::Inferred),
            ("traces[].extra.injection_status", Source::Inferred),
            ("traces[].extra.calibration_level", Source::Inferred),
            ("traces[].extra.processing_method", Source::Inferred),
            ("traces[].extra.instrument_method", Source::Inferred),
            ("traces[].extra.dilution_factor", Source::Inferred),
            ("traces[].extra.weight", Source::Inferred),
            ("traces[].extra.vendor_peaks", Source::Inferred),
            ("tables[].columns", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut by_member: BTreeMap<&str, &crate::chromeleon::ArchiveItem> = BTreeMap::new();
        for it in &self.header.items {
            if let Some(m) = &it.member {
                by_member.insert(m.as_str(), it);
            }
        }
        Ok(self
            .zip
            .members
            .iter()
            .map(|m| {
                let it = by_member.get(m.name.as_str());
                let kind = match it.map(|i| i.item_type.as_str()) {
                    Some(SIGNAL_ITEM) => "signal",
                    Some(SEQUENCE_ITEM) => "sequence",
                    Some(MS_RAW_ITEM) => "ms-raw",
                    Some(_) => "item",
                    None if m.name.eq_ignore_ascii_case("header.xml") => "manifest",
                    None => "member",
                };
                LsEntry {
                    kind: kind.into(),
                    name: m.name.clone(),
                    offset: None,
                    size: Some(m.size),
                    image: None,
                    details: match it {
                        Some(i) => json!({"item": i.name, "item_type": i.item_type,
                                          "compressed_size": m.compressed_size}),
                        None => json!({"compressed_size": m.compressed_size}),
                    },
                }
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            CHROMELEON_ID,
            "image planes",
            "Chromeleon archives hold chromatograms: use `openreadout analyze chromatogram --trace N`, `analyze peaks`, or `export --to csv`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (Chromeleon traces have one sweep)"
            )));
        }
        if self
            .signals
            .get(index as usize)
            .is_some_and(|s| s.field.is_some())
        {
            return self.read_field(index, first_sample, max_samples);
        }
        let expect = self
            .signals
            .get(index as usize)
            .and_then(Self::grid)
            .map(|g| g.2);
        let (name, desc_irregular) = self
            .signals
            .get(index as usize)
            .map(|s| {
                (
                    self.name_of(s),
                    s.desc.as_ref().is_some_and(|d| !d.is_regular()),
                )
            })
            .unwrap_or_default();
        let decoded = self.decoded(index)?;
        let total = decoded.len() as u64;
        if expect.is_some_and(|n| n != total) {
            return Err(Error::corrupt(
                CHROMELEON_ID,
                format!(
                    "{name}: the member decodes to {total} points, the sequence file says {}",
                    expect.unwrap_or(0)
                ),
            ));
        }
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let n = max_samples.min(total - first_sample);
        let (start, end) = (first_sample as usize, (first_sample + n) as usize);
        let values: Vec<f64> = if let Some(v) = &decoded.float_values {
            v[start..end].to_vec()
        } else {
            let scale = decoded.scale();
            decoded.raw[start..end]
                .iter()
                .map(|&v| v as f64 * scale)
                .collect()
        };
        let channels = if desc_irregular || decoded.times.is_some() {
            vec![(start..end).map(|i| decoded.time(i)).collect(), values]
        } else {
            vec![values]
        };
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let infos = self.table_infos();
        let Some(info) = infos.get(index as usize) else {
            return Err(Error::Usage(format!(
                "table {index} not found (this archive has {} tables)",
                infos.len()
            )));
        };
        let n = info.row_count as usize;
        let start = (first_row as usize).min(n);
        let end = start
            .saturating_add(max_rows.min(usize::MAX as u64) as usize)
            .min(n);
        let columns = if info.name.as_deref() == Some("vendor_peaks") {
            self.vendor_rows(start, end)
        } else {
            self.injection_rows(start, end)
        };
        Ok(Table {
            table: index,
            first_row: start as u64,
            columns,
        })
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self
            .attached
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let (name, kind) = self.attachment_name(a);
                let (size, magic) = match a {
                    Attached::Audit { item } => {
                        let head = self.header.items[*item]
                            .member
                            .as_deref()
                            .and_then(|m| self.zip.get(m).cloned())
                            .and_then(|m| self.zip.read(&m).ok());
                        (
                            head.as_deref().and_then(blob_stated_size),
                            head.as_deref().map(blob_magic),
                        )
                    }
                    Attached::InstrumentMethod(k) => {
                        let b = &self.contents.instrument_methods[*k].blob;
                        (
                            blob_stated_size(b).or_else(|| unpack_blob(b).ok().map(|x| x.len())),
                            Some(blob_magic(b)),
                        )
                    }
                    Attached::ProcessingMethod(k) => {
                        let b = self.contents.processing_methods[*k]
                            .1
                            .as_deref()
                            .unwrap_or_default();
                        (blob_stated_size(b), Some(blob_magic(b)))
                    }
                };
                let mut extra = BTreeMap::new();
                extra.insert("kind".into(), json!(kind));
                if let Some(m) = magic {
                    extra.insert("stored_as".into(), json!(format!("{m} (LZMA2)")));
                }
                AttachmentInfo {
                    index: i as u32,
                    name,
                    content_type: "application/xml".into(),
                    extension: "xml".into(),
                    offset: None,
                    size: size.unwrap_or(0) as u64,
                    extra,
                }
            })
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let a = self.attached.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "attachment {index} not found (this archive has {})",
                self.attached.len()
            ))
        })?;
        self.attachment_bytes(&a).map_err(|e| {
            Error::corrupt(
                CHROMELEON_ID,
                format!("{}: {e}", self.attachment_name(&a).0),
            )
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), CHROMELEON_ID);
        r.performed("container: zip central directory; every member inflated and checked against its CRC-32");
        r.performed("header.xml parsed; every item's member present");
        r.performed("sequence file: signal descriptions matched to signals by file id");
        r.performed("signals: sections walked, every PtsLDiff block decoded (version, rate, scale, terminator, no gaps); point count, first time, step, minimum and maximum against the sequence file's description; SignHdr name against header.xml");
        for m in self.zip.members.clone() {
            if m.size > MAX_SIGNAL_BYTES {
                // e.g. an embedded MS .raw of gigabytes: not inflated here
                r.push(Finding::info(
                    "member_not_verified",
                    format!(
                        "{} ({} bytes) is larger than {MAX_SIGNAL_BYTES} bytes and was not inflated",
                        m.name, m.size
                    ),
                ));
            } else if let Err(e) = self.zip.read(&m) {
                r.push(Finding::error("bad_member", e.to_string()));
            }
        }
        for it in &self.header.items {
            if let Some(m) = &it.member
                && self.zip.get(m).is_none()
            {
                r.push(Finding::error(
                    "missing_member",
                    format!(
                        "{} ({}): member {m} is not in the archive",
                        it.name, it.item_type
                    ),
                ));
            }
        }
        let (mut decoded, mut matched, mut timed) = (0usize, 0usize, 0usize);
        // Chromeleon's stored peaks recomputed on our signal
        let (mut stored, mut reproduced) = (0usize, 0usize);
        let mut off_peaks: Vec<String> = Vec::new();
        let (mut fields, mut fields_ok, mut not_start) = (0usize, 0usize, 0usize);
        for (ti, s) in self.signals.clone().iter().enumerate() {
            let name = self.name_of(s);
            if let Some(f) = &s.field {
                fields += 1;
                match self.check_field(ti as u32, f) {
                    Ok(()) => fields_ok += 1,
                    Err(fd) => r.push(fd),
                }
                continue;
            }
            let member = self.header.items[s.item].member.clone();
            let d = match Self::decode_member(&self.zip, member.as_deref()) {
                Ok(d) => d,
                Err(SignalError::Corrupt(e)) => {
                    r.push(Finding::error("bad_signal", format!("{name}: {e}")));
                    continue;
                }
                Err(SignalError::Unsupported(e)) => {
                    r.push(Finding::warning(
                        "signal_not_decoded",
                        format!("{name}: {e}"),
                    ));
                    continue;
                }
            };
            decoded += 1;
            if let Some(Ok(Some(res))) = s
                .chrom
                .and_then(|c| self.contents.chromatograms.get(c))
                .map(|c| &c.results)
            {
                let values = d.values();
                let times: Vec<f64> = (0..d.len()).map(|i| d.time(i)).collect();
                for (k, p) in res.peaks.iter().enumerate() {
                    stored += 1;
                    match integrate_stored_peak(&values, &times, res, k) {
                        // relative 1e-6; a zero stored height against a rounding residue
                        // is compared with the signal's magnitude at the apex
                        Some((a, h))
                            if (a - p.area).abs() <= 1e-6 * p.area.abs().max(1e-12)
                                && (h - p.height).abs()
                                    <= 1e-6 * p.height.abs().max(1e-6 * p.apex.1.abs()).max(1e-12) =>
                        {
                            reproduced += 1;
                        }
                        Some((a, h)) => off_peaks.push(format!(
                            "{name} peak {} at {:.3} min: stored area {} height {}, ours {a} {h}",
                            k + 1,
                            p.retention_min,
                            p.area,
                            p.height
                        )),
                        None => off_peaks.push(format!(
                            "{name} peak {} at {:.3} min: its limits are not on the signal's samples or its baseline is not one straight line",
                            k + 1,
                            p.retention_min
                        )),
                    }
                }
            }
            if let Some(h) = &d.header {
                if h.name != self.header.items[s.item].name {
                    r.push(Finding::warning(
                        "signal_name_mismatch",
                        format!("{name}: the member's header names signal {}", h.name),
                    ));
                }
                // the file id names the local start time, SignHdr the UTC one: they differ by a
                // time-zone offset (whole quarter hours, at most 14 h)
                let fid = self.header.items[s.item].file_id.as_deref();
                let inject = s
                    .details
                    .and_then(|i| self.contents.injections.get(i))
                    .and_then(|d| d.inject_time.as_deref());
                let is_start = fid.and_then(|f| file_id_is_start(f, inject));
                if is_start == Some(false) {
                    not_start += 1;
                }
                let local = fid
                    .and_then(file_id_time)
                    .filter(|_| s.injection.is_some() && is_start != Some(false))
                    .and_then(|t| iso8601_to_unix(&format!("{t}Z")));
                let utc = iso8601_to_unix(&filetime_to_iso8601(h.start_filetime));
                if let (Some(l), Some(u)) = (local, utc) {
                    let off = l - u;
                    let zone = (off / 900.0).round() * 900.0;
                    if (off - zone).abs() > 2.0 || zone.abs() > 14.0 * 3600.0 {
                        r.push(Finding::warning(
                            "start_time_mismatch",
                            format!(
                                "{name}: the member's start {} (UTC) is not a time-zone offset from its file id's local time",
                                filetime_to_iso8601(h.start_filetime)
                            ),
                        ));
                    } else {
                        timed += 1;
                    }
                }
            } else {
                r.push(Finding::info(
                    "signal_header_layout",
                    format!("{name}: the SignHdr section has a layout not seen before (name and start time not read)"),
                ));
            }
            let Some(desc) = &s.desc else { continue };
            let mut off = Vec::new();
            if desc.points != Some(d.len() as u64) {
                off.push(format!(
                    "{} points, the sequence file {:?}",
                    d.len(),
                    desc.points
                ));
            }
            if (d.start_min - desc.time.min).abs() > 1e-6 {
                off.push(format!(
                    "first point at {} min, the sequence file {} min",
                    d.start_min, desc.time.min
                ));
            }
            let last = d.time(d.len().saturating_sub(1));
            if (last - desc.time.max).abs() > 1e-6 {
                off.push(format!(
                    "last point at {last} min, the sequence file {} min",
                    desc.time.max
                ));
            }
            if desc
                .scale
                .is_some_and(|k| (k - d.scale()).abs() > 1e-12 * k.abs().max(1e-300))
            {
                off.push(format!(
                    "scale {} (block header), the sequence file {:?}",
                    d.scale(),
                    desc.scale
                ));
            }
            if !off.is_empty() {
                r.push(Finding::error(
                    "signal_mismatch",
                    format!("{name}: {}", off.join("; ")),
                ));
                continue;
            }
            let v = d.values();
            let (lo, hi) = v
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| {
                    (a.min(x), b.max(x))
                });
            let tol = 1e-9 * desc.signal.max.abs().max(desc.signal.min.abs()).max(1.0);
            if !v.is_empty()
                && (desc.signal.max != 0.0 || desc.signal.min != 0.0)
                && ((lo - desc.signal.min).abs() > tol || (hi - desc.signal.max).abs() > tol)
            {
                r.push(Finding::warning(
                    "stored_range_mismatch",
                    format!(
                        "{name}: decoded range {lo}–{hi}, the sequence file records {}–{}",
                        desc.signal.min, desc.signal.max
                    ),
                ));
            } else {
                matched += 1;
            }
        }
        r.performed(format!(
            "{decoded} of {} signals decoded; {matched} equal the sequence file's recorded minimum and maximum; {timed} start times (SignHdr, UTC) a time-zone offset from their file id's local time",
            self.signals.len() - fields
        ));
        if not_start > 0 {
            r.push(Finding::info(
                "file_id_not_start",
                format!(
                    "{not_start} signal(s) have a file id whose time is not their injection's inject time (data copied or derived later): their start time is not compared"
                ),
            ));
        }
        if fields > 0 {
            r.performed(format!(
                "spectral fields (3D): {fields_ok} of {fields} decoded whole (every spectrum on one wavelength grid; spectrum count, first and last time, minimum and maximum equal to the sequence file's description)"
            ));
        }
        if self.derived > 0 {
            r.push(Finding::info(
                "derived_signal",
                format!(
                    "{} signal(s) have no stored points (channels Chromeleon derives on demand, e.g. extracted-ion chromatograms of MS data)",
                    self.derived
                ),
            ));
        }
        if stored > 0 {
            r.performed(format!(
                "Chromeleon's stored results: {reproduced} of {stored} stored peaks reproduced on our decoded signal (area and height within 1e-6 relative, between the stored limits and under the stored baseline)"
            ));
        }
        if !off_peaks.is_empty() {
            let n = off_peaks.len();
            off_peaks.truncate(10);
            r.push(Finding::warning(
                "vendor_peak_mismatch",
                format!(
                    "{n} of {stored} stored peaks are not reproduced on our decoded signal: {}",
                    off_peaks.join("; ")
                ),
            ));
        }
        for c in &self.contents.chromatograms {
            if let Err(e) = &c.results {
                let (code, f): (&str, fn(&str, String) -> Finding) = match e {
                    SignalError::Corrupt(_) => ("bad_results", Finding::error),
                    SignalError::Unsupported(_) => ("results_not_decoded", Finding::warning),
                };
                r.push(f(
                    code,
                    format!("{}: {e}", c.name.as_deref().unwrap_or("chromatogram")),
                ));
            }
        }
        // audit trails and methods: every compressed blob inflates to XML
        let (mut blobs, mut messages) = (0usize, 0usize);
        for a in self.attached.clone() {
            let name = self.attachment_name(&a).0;
            match self.attachment_bytes(&a) {
                Ok(b) => {
                    blobs += 1;
                    if let Attached::Audit { .. } = a {
                        match std::str::from_utf8(&b)
                            .map_err(|e| e.to_string())
                            .and_then(audit_messages)
                        {
                            Ok(m) => messages += m.len(),
                            Err(e) => {
                                r.push(Finding::warning("bad_audit_trail", format!("{name}: {e}")));
                            }
                        }
                    }
                }
                Err(e) => r.push(Finding::error("bad_blob", format!("{name}: {e}"))),
            }
        }
        if !self.attached.is_empty() {
            r.performed(format!(
                "{blobs} of {} audit trails and methods decompressed (LZMA2); {messages} audit-trail messages parsed",
                self.attached.len()
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn file_id_times() {
        assert_eq!(
            file_id_time("2025\\11\\16\\164901056.raw").as_deref(),
            Some("2025-11-16T16:49:01.056")
        );
        assert_eq!(file_id_time("2016\\04\\12\\210905_0.raw").as_deref(), None);
        assert_eq!(file_id_time("x.raw"), None);
        assert_eq!(file_id_time("2025\\13\\16\\164901056.raw"), None);
    }

    #[test]
    fn filetime_is_utc_iso() {
        // 2025-11-16T15:49:01Z, the start of a corpus signal
        assert!(filetime_to_iso8601(134_077_817_410_318_823).starts_with("2025-11-16T15:49:01"));
    }
}
