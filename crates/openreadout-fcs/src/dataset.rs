//! `Dataset` implementation: normalized tables, vendor keywords, listing, event reads, checks.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, Table, TableInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::crc::{Crc16, CrcField, read_crc_field};
use crate::datetime::{acquisition_span, parse_date, parse_timestamp};
use crate::file::{DataSet, FcsFile, read_at};
use crate::keywords::{Keyword, parse_float, parse_uint};
use crate::layout::{
    ByteOrder, DataType, FieldWidth, Mode, SpilloverMatrix, decode_fixed, decode_free_ascii,
    parse_spillover, storage_dtype,
};
use crate::{FORMAT_ID, FcsReader};

/// Longest vendor keyword value copied into `info` (the full value is in `info --view full`'s
/// vendor tree).
const MAX_INFO_VALUE: usize = 1024;

/// An opened FCS file.
#[derive(Debug)]
pub struct FcsDataset {
    path: PathBuf,
    file: FcsFile,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
}

impl FcsDataset {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        Ok(FcsDataset {
            path: input.path().to_path_buf(),
            file: FcsFile::open_in(input.fs(), input.path())?,
            handle: None,
            fs: input.fs().clone(),
        })
    }

    /// The parsed data sets (for library users).
    pub fn data_sets(&self) -> &[DataSet] {
        &self.file.data_sets
    }

    fn handle(&mut self) -> Result<&mut SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }
}

/// The spillover matrix of a data set: `$SPILLOVER` (FCS 3.1), else `SPILL` (same layout, written
/// by BD FACSDiva and others) when its names are this data set's `$PnN`, else `$COMP` (FCS 3.0).
pub fn spillover(ds: &DataSet) -> Option<SpilloverMatrix> {
    if let Some(m) = ds
        .keyword("$SPILLOVER")
        .and_then(|v| parse_spillover("$SPILLOVER", v, false))
    {
        return Some(m);
    }
    for key in ["$SPILL", "SPILL"] {
        if let Some(m) = ds.keyword(key).and_then(|v| parse_spillover(key, v, false))
            && m.parameters
                .iter()
                .all(|n| ds.parameters.iter().any(|p| &p.short_name == n))
        {
            return Some(m);
        }
    }
    ds.keyword("$COMP")
        .and_then(|v| parse_spillover("$COMP", v, true))
}

/// Group a non-`$` keyword under a prefix, mechanically (no meaning is assigned):
/// a namespace before an inner `$` (`FJ$ACQSTATE` → `FJ`), a leading symbol (`@SAMPLEID1` → `@`),
/// `Pn…` parameter keywords → `Pn`, letters followed by digits (`LASER1NAME` → `LASERn`),
/// otherwise the first word (`CST SETUP DATE` → `CST`, `ANALOG_COMP` → `ANALOG`).
pub fn keyword_prefix(name: &str) -> String {
    let n = name.trim();
    if let Some(pos) = n.find('$')
        && pos > 0
    {
        return n[..pos].trim_end_matches('_').to_string();
    }
    let first = n.chars().next().unwrap_or(' ');
    if !first.is_ascii_alphanumeric() {
        return first.to_string();
    }
    let letters: String = n.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = &n[letters.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if !letters.is_empty() && !digits.is_empty() {
        let after = &rest[digits.len()..];
        if letters.eq_ignore_ascii_case("P")
            && after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            return "Pn".into();
        }
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            || after.is_empty()
        {
            return format!("{}n", letters.to_ascii_uppercase());
        }
    }
    n.split([' ', '_', '-'])
        .next()
        .unwrap_or(n)
        .to_ascii_uppercase()
}

fn truncated_value(v: &str) -> Value {
    if v.len() <= MAX_INFO_VALUE {
        Value::String(v.to_string())
    } else {
        let mut cut = MAX_INFO_VALUE;
        while !v.is_char_boundary(cut) {
            cut -= 1;
        }
        Value::String(format!(
            "{}… ({} bytes; full value in `info --view full` vendor tree)",
            &v[..cut],
            v.len()
        ))
    }
}

fn vendor_keywords(ds: &DataSet) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut groups: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let supplemental = ds.supplemental.as_ref().and_then(|s| s.keywords.as_ref());
    let all = ds.text.keywords.entries.iter().chain(
        supplemental
            .into_iter()
            .flat_map(|k| k.keywords.entries.iter()),
    );
    for Keyword { name, value } in all {
        if name.starts_with('$') {
            continue;
        }
        groups
            .entry(keyword_prefix(name))
            .or_default()
            .insert(name.clone(), truncated_value(value));
    }
    groups
}

fn put(m: &mut BTreeMap<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(k.into(), v);
    }
}

fn text_value(ds: &DataSet, k: &str) -> Option<Value> {
    ds.keyword(k)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| Value::String(s.to_string()))
}

fn num_value(ds: &DataSet, k: &str) -> Option<Value> {
    ds.keyword(k).and_then(parse_float).map(|f| json!(f))
}

fn column_info(ds: &DataSet, i: usize) -> ColumnInfo {
    let p = &ds.parameters[i];
    let mut extra = BTreeMap::new();
    put(
        &mut extra,
        "bits",
        p.width.map(|w| match w {
            FieldWidth::Fixed(b) => json!(b),
            FieldWidth::FreeFormat => json!("*"),
        }),
    );
    put(
        &mut extra,
        "range_keyword",
        p.range_text.clone().map(Value::String),
    );
    let p_type = p.data_type.or(ds.data_type);
    if p_type == Some(DataType::Integer) {
        let full_bits = match p.width {
            Some(FieldWidth::Fixed(b)) => b.min(64),
            _ => 64,
        };
        let mask = p.bit_mask();
        let full = if full_bits >= 64 {
            u64::MAX
        } else {
            (1u64 << full_bits) - 1
        };
        if mask != full {
            extra.insert("bit_mask".into(), json!(mask));
        }
    }
    put(
        &mut extra,
        "amplification",
        p.amplification
            .map(|[d, o]| json!({"decades": d, "offset": o})),
    );
    put(&mut extra, "gain", p.gain.map(|g| json!(g)));
    put(&mut extra, "detector_voltage", p.voltage.map(|v| json!(v)));
    if !p.wavelengths_nm.is_empty() {
        extra.insert("excitation_wavelength_nm".into(), json!(p.wavelengths_nm));
    }
    put(
        &mut extra,
        "excitation_power_mw",
        p.power_mw.map(|v| json!(v)),
    );
    put(&mut extra, "filter", p.filter.clone().map(Value::String));
    put(
        &mut extra,
        "detector_type",
        p.detector_type.clone().map(Value::String),
    );
    put(
        &mut extra,
        "percent_emitted",
        p.percent_emitted.map(|v| json!(v)),
    );
    put(
        &mut extra,
        "display_scale",
        p.display.clone().map(Value::String),
    );
    put(
        &mut extra,
        "calibration",
        p.calibration.clone().map(Value::String),
    );
    for (k, v) in crate::vendor::column_extras(ds, p, crate::vendor::family(ds)) {
        extra.entry(k).or_insert(v);
    }
    if let Some(t) = p.data_type {
        extra.insert("datatype".into(), json!(t.code()));
    }
    let range = p.range.map(|r| {
        if p_type == Some(DataType::Integer) {
            [0.0, (r - 1.0).max(0.0)]
        } else {
            [0.0, r]
        }
    });
    ColumnInfo {
        index: i as u32,
        name: p.short_name.clone(),
        label: p.label.clone(),
        dtype: storage_dtype(p_type, p.width).to_string(),
        unit: None,
        range,
        extra,
    }
}

fn table_info(ds: &DataSet) -> TableInfo {
    let mut extra = BTreeMap::new();
    extra.insert("fcs_version".into(), json!(ds.header.version));
    extra.insert("data_set_offset".into(), json!(ds.offset));
    put(
        &mut extra,
        "datatype",
        ds.data_type.map(|d| json!(d.code())),
    );
    put(
        &mut extra,
        "byte_order",
        ds.byte_order.as_ref().map(|b| json!(b.name())),
    );
    put(&mut extra, "mode", ds.mode.map(|m| json!(m.code())));
    put(
        &mut extra,
        "event_width_bytes",
        ds.event_width().map(|w| json!(w)),
    );
    let mut instrument = serde_json::Map::new();
    if let Some(v) = text_value(ds, "$CYT") {
        instrument.insert("model".into(), v);
    }
    if let Some(v) = text_value(ds, "$CYTSN") {
        instrument.insert("serial_number".into(), v);
    }
    if !instrument.is_empty() {
        extra.insert("instrument".into(), Value::Object(instrument));
    }
    put(&mut extra, "software", text_value(ds, "CREATOR"));
    put(&mut extra, "system", text_value(ds, "$SYS"));
    let date = ds.keyword("$DATE");
    put(
        &mut extra,
        "acquisition_date",
        date.and_then(parse_date).map(|d| json!(d.iso())),
    );
    let (mut start, mut end) = acquisition_span(date, ds.keyword("$BTIM"), ds.keyword("$ETIM"));
    // FCS 3.2: ISO 8601 date-times, preferred over the deprecated $DATE/$BTIM/$ETIM.
    if let Some(v) = ds
        .keyword("$BEGINDATETIME")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        start = Some(v.to_string());
    }
    if let Some(v) = ds
        .keyword("$ENDDATETIME")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        end = Some(v.to_string());
    }
    put(&mut extra, "acquisition_start", start.map(Value::String));
    put(&mut extra, "acquisition_end", end.map(Value::String));
    for (k, key) in [
        ("$FIL", "file_name"),
        ("$SRC", "source"),
        ("$EXP", "experimenter"),
        ("$OP", "operator"),
        ("$INST", "institution"),
        ("$SMNO", "specimen"),
        ("$CELLS", "cells"),
        ("$PROJ", "project"),
        ("$COM", "comment"),
        ("$PLATEID", "plate_id"),
        ("$PLATENAME", "plate_name"),
        ("$WELLID", "well_id"),
        ("$ORIGINALITY", "originality"),
        ("$LAST_MODIFIER", "last_modifier"),
        ("$CARRIERID", "carrier_id"),
        ("$CARRIERTYPE", "carrier_type"),
        ("$LOCATIONID", "location_id"),
        ("$FLOWRATE", "flow_rate"),
        ("$UNSTAINEDCENTERS", "unstained_centers"),
        ("$UNSTAINEDINFO", "unstained_info"),
    ] {
        put(&mut extra, key, text_value(ds, k));
    }
    put(
        &mut extra,
        "last_modified",
        ds.keyword("$LAST_MODIFIED")
            .and_then(parse_timestamp)
            .map(Value::String),
    );
    put(&mut extra, "volume_nl", num_value(ds, "$VOL"));
    put(&mut extra, "timestep_s", num_value(ds, "$TIMESTEP"));
    put(
        &mut extra,
        "events_lost",
        ds.keyword("$LOST").and_then(parse_uint).map(|v| json!(v)),
    );
    put(
        &mut extra,
        "events_aborted",
        ds.keyword("$ABRT").and_then(parse_uint).map(|v| json!(v)),
    );
    if let Some(tr) = ds.keyword("$TR")
        && let Some((p, t)) = tr.rsplit_once(',')
    {
        extra.insert(
            "trigger".into(),
            json!({"parameter": p.trim(), "threshold": parse_float(t).map_or_else(|| json!(t.trim()), |f| json!(f))}),
        );
    }
    if let Some(m) = spillover(ds) {
        extra.insert(
            "spillover".into(),
            json!({"keyword": m.keyword, "parameters": m.parameters, "matrix": m.values}),
        );
    }
    if let Some(s) = &ds.supplemental {
        extra.insert(
            "supplemental_text".into(),
            json!({"offset": s.range.begin, "size": s.range.byte_len(),
                   "keyword_count": s.keywords.as_ref().map(|k| k.keywords.len()), "note": s.note}),
        );
    }
    if let Some(a) = &ds.analysis {
        extra.insert(
            "analysis_segment".into(),
            json!({"offset": a.range.begin, "size": a.range.byte_len(),
                   "keyword_count": a.keywords.as_ref().map(|k| k.keywords.len()), "note": a.note}),
        );
    }
    if let Some(p) = crate::vendor::platform(ds) {
        extra.insert("platform".into(), p);
    }
    let groups = vendor_keywords(ds);
    if !groups.is_empty() {
        extra.insert("vendor_keywords".into(), json!(groups));
    }
    TableInfo {
        index: ds.index,
        name: ds
            .keyword("$FIL")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        row_count: ds.event_count.unwrap_or(0),
        columns: (0..ds.parameters.len())
            .map(|i| column_info(ds, i))
            .collect(),
        extra,
    }
}

fn keywords_json(entries: &[Keyword]) -> Value {
    let mut m = serde_json::Map::new();
    for k in entries {
        m.insert(k.name.clone(), Value::String(k.value.clone()));
    }
    Value::Object(m)
}

fn delimiter_name(d: u8) -> String {
    if d.is_ascii_graphic() {
        char::from(d).to_string()
    } else {
        format!("0x{d:02x}")
    }
}

impl Dataset for FcsDataset {
    fn info(&self) -> Result<FileInfo> {
        let tables: Vec<TableInfo> = self.file.data_sets.iter().map(table_info).collect();
        let mut notes = vec![
            "read_table/export return raw DATA values: not compensated, not scaled by $PnE/$PnG/$TIMESTEP; $DATATYPE/I/ values are masked per $PnR".to_string(),
        ];
        if tables.len() > 1 {
            notes.push(format!(
                "{} data sets chained by $NEXTDATA; each is one table",
                tables.len()
            ));
        }
        let errors = self
            .file
            .data_sets
            .iter()
            .flat_map(|d| d.findings.iter())
            .chain(self.file.chain_findings.iter())
            .any(|f| f.severity == openreadout_core::model::Severity::Error);
        if errors {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        if tables
            .iter()
            .any(|t| t.extra.contains_key("acquisition_start"))
        {
            notes.push(
                "acquisition_start/acquisition_end are local clock times from $DATE/$BTIM/$ETIM; FCS records no time zone, so they carry no zone designator".into(),
            );
        }
        if self
            .file
            .data_sets
            .iter()
            .any(|d| matches!(d.mode, Some(Mode::Correlated | Mode::Uncorrelated)))
        {
            notes.push("histogram-mode data sets ($MODE C/U) are described but not decoded".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file.file_len,
            format: FcsReader.descriptor(),
            format_version: self
                .file
                .data_sets
                .first()
                .map(|d| d.header.version.trim_start_matches("FCS").to_string()),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let sets: Vec<Value> = self
            .file
            .data_sets
            .iter()
            .map(|d| {
                json!({
                    "index": d.index,
                    "offset": d.offset,
                    "version": d.header.version,
                    "delimiter": delimiter_name(d.delimiter),
                    "header": {
                        "text": [d.header.text.begin, d.header.text.end],
                        "data": [d.header.data.begin, d.header.data.end],
                        "analysis": [d.header.analysis.begin, d.header.analysis.end],
                        "other": d.header.other.iter().map(|o| [o.begin, o.end]).collect::<Vec<_>>(),
                    },
                    "text": keywords_json(&d.text.keywords.entries),
                    "supplemental_text": d.supplemental.as_ref().and_then(|s| s.keywords.as_ref()).map(|k| keywords_json(&k.keywords.entries)),
                    "analysis": d.analysis.as_ref().and_then(|s| s.keywords.as_ref()).map(|k| keywords_json(&k.keywords.entries)),
                })
            })
            .collect();
        Ok(json!({ "data_sets": sets }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Spec),
            ("tables[].row_count", Source::Spec),
            ("tables[].name", Source::Spec),
            ("tables[].columns[].name", Source::Spec),
            ("tables[].columns[].label", Source::Spec),
            ("tables[].columns[].dtype", Source::Spec),
            ("tables[].columns[].range", Source::Spec),
            ("tables[].columns[].extra", Source::Spec),
            ("tables[].extra.datatype", Source::Spec),
            ("tables[].extra.byte_order", Source::Spec),
            ("tables[].extra.mode", Source::Spec),
            ("tables[].extra.instrument", Source::Spec),
            ("tables[].extra.software", Source::Inferred),
            ("tables[].extra.system", Source::Spec),
            ("tables[].extra.acquisition_date", Source::Spec),
            ("tables[].extra.acquisition_start", Source::Spec),
            ("tables[].extra.acquisition_end", Source::Spec),
            ("tables[].extra.source", Source::Spec),
            ("tables[].extra.experimenter", Source::Spec),
            ("tables[].extra.operator", Source::Spec),
            ("tables[].extra.spillover", Source::Spec),
            ("tables[].extra.spillover[keyword=SPILL]", Source::Inferred),
            ("tables[].extra.trigger", Source::Spec),
            ("tables[].extra.timestep_s", Source::Spec),
            ("tables[].extra.volume_nl", Source::Spec),
            ("tables[].extra.vendor_keywords", Source::Inferred),
            ("tables[].extra.platform", Source::Inferred),
            ("tables[].columns[].extra.channel_kind", Source::Inferred),
            ("tables[].columns[].extra.measure", Source::Inferred),
            ("tables[].columns[].extra.laser", Source::Inferred),
            ("tables[].columns[].extra.metal_tag", Source::Inferred),
            ("tables[].columns[].extra.marker", Source::Inferred),
            ("tables[].extra.supplemental_text", Source::Spec),
            ("tables[].extra.analysis_segment", Source::Spec),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        let seg = |name: &str, begin: u64, len: Option<u64>, i: u32, details: Value| LsEntry {
            kind: "segment".into(),
            name: name.into(),
            offset: Some(begin),
            size: len,
            image: None,
            details: {
                let mut d = details;
                if let Value::Object(m) = &mut d {
                    m.insert("data_set".into(), json!(i));
                }
                d
            },
        };
        for d in &self.file.data_sets {
            out.push(seg(
                "HEADER",
                d.offset,
                Some(d.header.header_len),
                d.index,
                json!({"version": d.header.version, "other_segments": d.header.other.len()}),
            ));
            out.push(seg(
                "TEXT",
                d.text_range.begin,
                d.text_range.byte_len(),
                d.index,
                json!({"delimiter": delimiter_name(d.delimiter), "keywords": d.text.keywords.len()}),
            ));
            if let Some(s) = &d.supplemental {
                out.push(seg(
                    "STEXT",
                    s.range.begin,
                    s.range.byte_len(),
                    d.index,
                    json!({"keywords": s.keywords.as_ref().map(|k| k.keywords.len()), "note": s.note}),
                ));
            }
            if let Some(r) = d.data_range {
                out.push(seg(
                    "DATA",
                    r.begin,
                    r.byte_len(),
                    d.index,
                    json!({"offsets_from": d.data_source.map(crate::file::OffsetSource::name), "events": d.event_count,
                           "parameters": d.parameters.len(), "event_width_bytes": d.event_width(),
                           "datatype": d.data_type.map(DataType::code), "byte_order": d.byte_order.as_ref().map(ByteOrder::name)}),
                ));
            }
            if let Some(a) = &d.analysis {
                out.push(seg(
                    "ANALYSIS",
                    a.range.begin,
                    a.range.byte_len(),
                    d.index,
                    json!({"keywords": a.keywords.as_ref().map(|k| k.keywords.len()), "note": a.note}),
                ));
            }
            for (i, o) in d.header.other.iter().enumerate() {
                if let Some(a) = o.absolute(d.offset) {
                    out.push(seg(
                        &format!("OTHER{}", i + 1),
                        a.begin,
                        a.byte_len(),
                        d.index,
                        json!({}),
                    ));
                }
            }
            let crc_at = d.last_segment_end() + 1;
            if crc_at + 8 <= self.file.file_len {
                out.push(seg("CRC", crc_at, Some(8), d.index, json!({})));
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "FCS files hold event tables, not images: use `openreadout export FILE --to csv` or the openreadout_table MCP tool.",
        ))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let ds = self
            .file
            .data_sets
            .get(index as usize)
            .cloned()
            .ok_or_else(|| {
                Error::Usage(format!(
                    "table index {index} out of range (0..{})",
                    self.file.data_sets.len()
                ))
            })?;
        match ds.mode {
            Some(Mode::List) => {}
            Some(m) => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("$MODE/{}/ (histogram data)", m.code()),
                    "Only list-mode ($MODE/L/) DATA is decoded; histogram modes are deprecated in FCS 3.1.",
                ));
            }
            None => {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("data set {index}: $MODE is missing or invalid"),
                ));
            }
        }
        let dt = ds.data_type.ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("data set {index}: $DATATYPE is missing or invalid"),
            )
        })?;
        let order = ds.byte_order.clone().ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("data set {index}: $BYTEORD is missing"))
        })?;
        if let ByteOrder::Mixed(s) = &order
            && dt != DataType::Ascii
        {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("$BYTEORD/{s}/"),
                "Only little-endian (1,2,3,4) and big-endian (4,3,2,1) byte orders are decoded.",
            ));
        }
        let total = ds.event_count.ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("data set {index}: $TOT is missing"))
        })?;
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the last event ({total} events)"
            )));
        }
        let rows = max_rows.min(total - first_row);
        let empty = Table {
            table: index,
            first_row,
            columns: vec![Vec::new(); ds.parameters.len()],
        };
        if rows == 0 {
            return Ok(empty);
        }
        let data = ds.data_range.ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("data set {index}: DATA segment not located"),
            )
        })?;
        let file_len = self.file.file_len;
        let path = self.path.clone();
        if ds.is_free_format() {
            let len = data.byte_len().unwrap_or(0);
            if data.end >= file_len {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    data.begin,
                    "DATA extends past the end of the file (truncated)",
                ));
            }
            let bytes = read_at(self.handle()?, &path, data.begin, len, file_len)?;
            let n =
                usize::try_from(total).map_err(|_| Error::corrupt(FORMAT_ID, "too many events"))?;
            let cols = decode_free_ascii(&bytes, n, ds.parameters.len())
                .map_err(|e| Error::corrupt_at(FORMAT_ID, data.begin, e))?;
            let a = first_row as usize;
            let b = a + rows as usize;
            return Ok(Table {
                table: index,
                first_row,
                columns: cols.into_iter().map(|c| c[a..b].to_vec()).collect(),
            });
        }
        let width = ds.event_width().ok_or_else(|| {
            let bad = ds
                .parameters
                .iter()
                .find(|p| p.byte_width(p.data_type.unwrap_or(dt)).is_none())
                .map_or(0, |p| p.number);
            Error::unsupported(
                FORMAT_ID,
                format!("$P{bad}B that is not a whole number of bytes"),
                "Bit-packed integer fields are not decoded yet.",
            )
        })?;
        let begin = first_row
            .checked_mul(width)
            .and_then(|o| o.checked_add(data.begin))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "DATA offset overflow"))?;
        let len = rows
            .checked_mul(width)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "DATA length overflow"))?;
        let seg_end = data.end.saturating_add(1);
        if begin + len > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                begin,
                format!(
                    "events {first_row}..{} need bytes up to {} but the file has {file_len} bytes (truncated)",
                    first_row + rows,
                    begin + len
                ),
            ));
        }
        if begin + len > seg_end {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                begin,
                format!(
                    "events {first_row}..{} run past the end of the DATA segment",
                    first_row + rows
                ),
            ));
        }
        let bytes = read_at(self.handle()?, &path, begin, len, file_len)?;
        let cols =
            decode_fixed(&bytes, rows as usize, dt, &order, &ds.parameters).map_err(|e| {
                if e.contains("not decoded")
                    || e.contains("byte-aligned")
                    || e.contains("wider than")
                {
                    Error::unsupported(
                        FORMAT_ID,
                        e,
                        "See `openreadout self formats` for known gaps.",
                    )
                } else {
                    Error::corrupt_at(FORMAT_ID, begin, e)
                }
            })?;
        Ok(Table {
            table: index,
            first_row,
            columns: cols,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("HEADER of every data set: FCS version identifier and ASCII segment offsets");
        r.performed(
            "primary TEXT: delimiter, keyword-value syntax, escaped delimiters, duplicate keywords",
        );
        r.performed("required keywords present ($BYTEORD, $DATATYPE, $MODE, $PAR, $TOT, $PnB, $PnN, $PnR, $PnE; segment keywords for 3.x)");
        r.performed("$PnB consistent with $DATATYPE; $PnN unique");
        r.performed(
            "DATA offsets: HEADER vs $BEGINDATA/$ENDDATA, and DATA length == $TOT × event width",
        );
        r.performed("every segment (TEXT, supplemental TEXT, DATA, ANALYSIS) lies inside the file; $NEXTDATA chain");
        r.performed("CRC field after the last segment, verified when one is stored");
        for d in &self.file.data_sets {
            for f in &d.findings {
                let mut f = f.clone();
                if self.file.data_sets.len() > 1 {
                    f.message = format!("data set {}: {}", d.index, f.message);
                }
                r.push(f);
            }
        }
        for f in &self.file.chain_findings {
            r.push(f.clone());
        }
        let file_len = self.file.file_len;
        let path = self.path.clone();
        let sets = self.file.data_sets.clone();
        for d in &sets {
            let crc_at = d.last_segment_end().saturating_add(1);
            let field = if crc_at + 8 <= file_len {
                read_crc_field(&read_at(self.handle()?, &path, crc_at, 8, file_len)?)
            } else {
                CrcField::Absent
            };
            let prefix = if sets.len() > 1 {
                format!("data set {}: ", d.index)
            } else {
                String::new()
            };
            match field {
                CrcField::Value(stored) => {
                    let mut crc = Crc16::new();
                    let mut pos = d.offset;
                    let f = self.handle()?;
                    while pos < crc_at {
                        let n = (crc_at - pos).min(1 << 20);
                        crc.update(&read_at(f, &path, pos, n, file_len)?);
                        pos += n;
                    }
                    let got = crc.finish();
                    if u32::from(got) == stored || u32::from(got.swap_bytes()) == stored {
                        r.push(
                            Finding::info("crc_ok", format!("{prefix}stored CRC {stored} matches"))
                                .at(crc_at),
                        );
                    } else {
                        r.push(
                            Finding::warning(
                                "crc_mismatch",
                                format!("{prefix}stored CRC {stored} != computed {got} (CRC-16/CCITT, bit-reflected, initial 0); writers differ in CRC details, so this is not treated as corruption"),
                            )
                            .at(crc_at),
                        );
                    }
                }
                CrcField::NotComputed => r.push(
                    Finding::info(
                        "crc_not_computed",
                        format!("{prefix}CRC field is 00000000 (the writer did not compute one)"),
                    )
                    .at(crc_at),
                ),
                CrcField::Absent | CrcField::Other => {}
            }
        }
        if self.file.data_sets.iter().all(|d| d.event_count == Some(0)) {
            r.push(Finding::warning(
                "no_events",
                "no data set holds any events",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(keyword_prefix("CST SETUP STATUS"), "CST");
        assert_eq!(keyword_prefix("CYTOMETER CONFIG NAME"), "CYTOMETER");
        assert_eq!(keyword_prefix("LASER1NAME"), "LASERn");
        assert_eq!(keyword_prefix("LASER2ASF"), "LASERn");
        assert_eq!(keyword_prefix("P5DISPLAY"), "Pn");
        assert_eq!(keyword_prefix("P12BS"), "Pn");
        assert_eq!(keyword_prefix("FJ$ACQSTATE"), "FJ");
        assert_eq!(keyword_prefix("FJ_$P1R"), "FJ");
        assert_eq!(keyword_prefix("GTI$SAMPLEID"), "GTI");
        assert_eq!(keyword_prefix("@SAMPLEID1"), "@");
        assert_eq!(keyword_prefix("ANALOG_COMP"), "ANALOG");
        assert_eq!(keyword_prefix("CREATOR"), "CREATOR");
        assert_eq!(keyword_prefix("APPLY COMPENSATION"), "APPLY");
    }
}
